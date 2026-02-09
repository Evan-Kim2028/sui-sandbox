//! Error context enrichment for debugging.
//!
//! Provides traits and utilities for adding execution context to errors
//! without prescribing fixes - just facts for debugging.
//!
//! # Design Philosophy
//!
//! Error context should be:
//! - **Factual**: Report what happened, not what to do
//! - **Neutral**: Describe the situation without judgment
//! - **DeFi-aware**: Understand complex multi-step transactions
//! - **Debuggable**: Provide enough information to investigate
//!
//! # Example
//!
//! ```
//! use sui_sandbox_core::error_context::{ExecutionContext, ObjectContext};
//!
//! let ctx = ExecutionContext::new()
//!     .at_command(3, "MoveCall 0x2::coin::split")
//!     .with_object(ObjectContext {
//!         id: "0x123...".into(),
//!         type_tag: Some("0x2::coin::Coin<0x2::sui::SUI>".into()),
//!         version: Some(42),
//!         owner: Some("0xabc...".into()),
//!         data_size: Some(40),
//!     });
//!
//! assert_eq!(ctx.command_index, Some(3));
//! assert_eq!(ctx.input_objects.len(), 1);
//! ```

use std::fmt;

/// Context that can be attached to any error for debugging.
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Which PTB command was executing (0-indexed)
    pub command_index: Option<usize>,

    /// Description of the command (e.g., "MoveCall 0x2::coin::split")
    pub command_description: Option<String>,

    /// Objects that were inputs to this operation
    pub input_objects: Vec<ObjectContext>,

    /// Packages that were loaded for this execution
    pub loaded_packages: Vec<String>,

    /// The transaction checkpoint/timestamp if replaying historical
    pub historical_checkpoint: Option<u64>,

    /// Any version mismatches detected
    pub version_notes: Vec<String>,

    /// Additional notes about the execution state
    pub notes: Vec<String>,
}

/// Context about a specific object involved in an error.
#[derive(Debug, Clone)]
pub struct ObjectContext {
    /// Object ID
    pub id: String,
    /// Type tag of the object
    pub type_tag: Option<String>,
    /// Object version
    pub version: Option<u64>,
    /// Owner address or "shared"/"immutable"
    pub owner: Option<String>,
    /// Size of BCS data (helps identify truncation issues)
    pub data_size: Option<usize>,
}

impl ExecutionContext {
    /// Create a new empty execution context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the command index and description.
    pub fn at_command(mut self, index: usize, description: impl Into<String>) -> Self {
        self.command_index = Some(index);
        self.command_description = Some(description.into());
        self
    }

    /// Add an object to the context.
    pub fn with_object(mut self, obj: ObjectContext) -> Self {
        self.input_objects.push(obj);
        self
    }

    /// Add a loaded package.
    pub fn with_package(mut self, package: impl Into<String>) -> Self {
        self.loaded_packages.push(package.into());
        self
    }

    /// Set the historical checkpoint for replay.
    pub fn with_checkpoint(mut self, checkpoint: u64) -> Self {
        self.historical_checkpoint = Some(checkpoint);
        self
    }

    /// Add a version-related note.
    pub fn with_version_note(mut self, note: impl Into<String>) -> Self {
        self.version_notes.push(note.into());
        self
    }

    /// Add a general note.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Check if this context has any information.
    pub fn is_empty(&self) -> bool {
        self.command_index.is_none()
            && self.input_objects.is_empty()
            && self.loaded_packages.is_empty()
            && self.historical_checkpoint.is_none()
            && self.version_notes.is_empty()
            && self.notes.is_empty()
    }
}

impl fmt::Display for ExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }

        writeln!(f, "Execution context:")?;

        if let Some(idx) = self.command_index {
            if let Some(ref desc) = self.command_description {
                writeln!(f, "  Command: #{} ({})", idx, desc)?;
            } else {
                writeln!(f, "  Command: #{}", idx)?;
            }
        }

        if let Some(checkpoint) = self.historical_checkpoint {
            writeln!(f, "  Checkpoint: {}", checkpoint)?;
        }

        if !self.input_objects.is_empty() {
            writeln!(f, "  Objects:")?;
            for obj in &self.input_objects {
                write!(f, "    - {}", obj.id)?;
                if let Some(ref t) = obj.type_tag {
                    write!(f, " ({})", t)?;
                }
                if let Some(v) = obj.version {
                    write!(f, " v{}", v)?;
                }
                if let Some(size) = obj.data_size {
                    write!(f, " [{} bytes]", size)?;
                }
                writeln!(f)?;
            }
        }

        if !self.loaded_packages.is_empty() {
            writeln!(f, "  Loaded packages:")?;
            for pkg in &self.loaded_packages {
                writeln!(f, "    - {}", pkg)?;
            }
        }

        if !self.version_notes.is_empty() {
            writeln!(f, "  Version notes:")?;
            for note in &self.version_notes {
                writeln!(f, "    - {}", note)?;
            }
        }

        if !self.notes.is_empty() {
            for note in &self.notes {
                writeln!(f, "  Note: {}", note)?;
            }
        }

        Ok(())
    }
}

impl ObjectContext {
    /// Create a new object context with just an ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            type_tag: None,
            version: None,
            owner: None,
            data_size: None,
        }
    }

    /// Set the type tag.
    pub fn with_type(mut self, type_tag: impl Into<String>) -> Self {
        self.type_tag = Some(type_tag.into());
        self
    }

    /// Set the version.
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    /// Set the owner.
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Set the data size.
    pub fn with_size(mut self, size: usize) -> Self {
        self.data_size = Some(size);
        self
    }
}

/// Get factual context about common abort codes.
/// Does NOT prescribe fixes - just explains what the code typically means.
pub fn get_abort_code_context(code: u64, module: &str) -> Option<String> {
    // Check module-specific codes first (more specific)
    // Coin-specific
    if module.contains("coin") {
        return match code {
            0 => Some("Coin value assertion (e.g., non-zero required)".into()),
            1 => Some("Insufficient coin balance".into()),
            _ => None,
        };
    }

    // Balance-specific
    if module.contains("balance") {
        return match code {
            0 => Some("Insufficient balance for operation".into()),
            1 => Some("Balance overflow".into()),
            _ => None,
        };
    }

    // Pool-specific (DeFi)
    if module.contains("pool") {
        return match code {
            3 => Some("Pool liquidity or tick range check".into()),
            4 => Some("Slippage tolerance exceeded".into()),
            _ => None,
        };
    }

    // Transfer-specific
    if module.contains("transfer") {
        return match code {
            0 => Some("Transfer permission denied".into()),
            _ => None,
        };
    }

    // Object-specific
    if module.contains("object") {
        return match code {
            0 => Some("Object ownership check failed".into()),
            _ => None,
        };
    }

    // Generic/common abort codes (fallback)
    match code {
        0 => Some("Assertion failed (generic)".into()),
        1 => Some("Arithmetic overflow or underflow".into()),
        257 => Some("Vector index out of bounds".into()),
        513 => Some("Version check failed - package version mismatch".into()),
        _ => None,
    }
}

/// Information about package upgrades.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageUpgradeInfo {
    /// Original package ID (what transactions reference)
    pub original_id: String,
    /// Current storage ID (where bytecode lives)
    pub storage_id: String,
    /// Version number
    pub version: u64,
}

// =============================================================================
// Command Execution Context (for PTB debugging)
// =============================================================================

/// Detailed context for command execution failures.
///
/// This provides everything needed to understand why a PTB command failed,
/// including the objects involved, gas consumed, and state at failure time.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CommandErrorContext {
    /// The command index that failed (0-indexed)
    pub command_index: usize,

    /// Type of command (e.g., "MoveCall", "SplitCoins", "MergeCoins", "TransferObjects")
    pub command_type: String,

    /// For MoveCall: the full function signature (package::module::function)
    pub function_signature: Option<String>,

    /// Type arguments passed to the call (if any)
    pub type_arguments: Vec<String>,

    /// Objects that were inputs to this command
    pub input_objects: Vec<ObjectSnapshot>,

    /// Gas consumed before this command failed
    pub gas_consumed_before_failure: u64,

    /// Commands that succeeded before this one
    pub prior_successful_commands: Vec<usize>,

    /// For coin operations: the actual balance(s) involved
    pub coin_balances: Option<CoinOperationContext>,

    /// For abort errors: detailed abort information
    pub abort_info: Option<TransactionAbortInfo>,
}

/// Snapshot of an object's state at a point in time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObjectSnapshot {
    /// Object ID
    pub id: String,
    /// Type tag of the object
    pub type_tag: String,
    /// Object version
    pub version: u64,
    /// Size of BCS data in bytes
    pub data_size: usize,
    /// Owner: "address:0x...", "shared", or "immutable"
    pub owner: String,
    /// Whether this object was modified by a prior command in this PTB
    pub modified_in_ptb: bool,
}

/// Context for coin-related operations (SplitCoins, MergeCoins).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoinOperationContext {
    /// The coin type (e.g., "0x2::sui::SUI")
    pub coin_type: String,

    /// For SplitCoins: the source coin balance
    pub source_balance: Option<u64>,

    /// For SplitCoins: the requested split amounts
    pub requested_splits: Option<Vec<u64>>,

    /// For MergeCoins: the destination coin balance
    pub destination_balance: Option<u64>,

    /// For MergeCoins: the source coin balances being merged
    pub source_balances: Option<Vec<u64>>,
}

/// Detailed information about a Move abort from transaction execution.
///
/// This struct captures abort information from gRPC transaction data,
/// including CleverError constant names when available.
///
/// Note: This is distinct from `errors::AbortInfo` which is used for
/// local VM execution diagnostics with call stacks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransactionAbortInfo {
    /// Module where the abort occurred (e.g., "0x2::coin")
    pub module: String,

    /// Function where the abort occurred
    pub function: String,

    /// The abort code
    pub abort_code: u64,

    /// The constant name used for this abort code (e.g., "E_INSUFFICIENT_BALANCE").
    /// Only available when the transaction was fetched via gRPC and the abort
    /// used a named constant (CleverError).
    pub constant_name: Option<String>,

    /// Human-readable interpretation of the abort code (if known)
    pub abort_meaning: Option<String>,

    /// Objects that were arguments to the aborting function
    pub involved_objects: Vec<String>,
}

/// Type alias for backward compatibility.
#[deprecated(since = "0.11.0", note = "Use TransactionAbortInfo instead")]
pub type AbortInfo = TransactionAbortInfo;

impl TransactionAbortInfo {
    /// Create AbortInfo from gRPC MoveAbort data.
    ///
    /// This extracts the constant_name from CleverError if available,
    /// and falls back to heuristic abort code meanings.
    pub fn from_grpc_move_abort(abort: &sui_transport::grpc::GrpcMoveAbort) -> Self {
        let module = abort.module.clone().unwrap_or_else(|| "unknown".into());
        let function = abort
            .function_name
            .clone()
            .unwrap_or_else(|| "unknown".into());

        // Prefer the CleverError constant_name, fall back to heuristic
        let abort_meaning = if abort.constant_name.is_some() {
            // If we have the constant name, use the rendered message if available
            abort.rendered_message.clone()
        } else {
            // Fall back to heuristic-based meaning
            get_abort_code_context(abort.abort_code, &module)
        };

        Self {
            module,
            function,
            abort_code: abort.abort_code,
            constant_name: abort.constant_name.clone(),
            abort_meaning,
            involved_objects: Vec::new(),
        }
    }
}

/// Snapshot of execution state at failure time.
///
/// Use this for post-mortem debugging to understand what state
/// the PTB was in when it failed.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ExecutionSnapshot {
    /// All objects that were loaded during execution
    pub objects: Vec<ObjectSnapshot>,

    /// Dynamic field entries that were accessed
    pub dynamic_fields_accessed: Vec<DynamicFieldAccess>,

    /// Commands that completed successfully
    pub successful_commands: Vec<CommandSummary>,

    /// Total gas consumed before failure
    pub total_gas_consumed: u64,
}

/// Record of a dynamic field access during execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DynamicFieldAccess {
    /// Parent object ID
    pub parent_id: String,
    /// Key type
    pub key_type: String,
    /// Value type (if known)
    pub value_type: Option<String>,
    /// Whether this was a read or write
    pub access_type: String,
}

/// Summary of a successfully executed command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandSummary {
    /// Command index
    pub index: usize,
    /// Command type
    pub command_type: String,
    /// Brief description
    pub description: String,
    /// Gas consumed by this command
    pub gas_consumed: u64,
    /// Objects created by this command
    pub objects_created: Vec<String>,
    /// Objects mutated by this command
    pub objects_mutated: Vec<String>,
}

impl CommandErrorContext {
    /// Create a new command error context.
    pub fn new(index: usize, command_type: impl Into<String>) -> Self {
        Self {
            command_index: index,
            command_type: command_type.into(),
            ..Default::default()
        }
    }

    /// Set the function signature for MoveCall errors.
    pub fn with_function(mut self, signature: impl Into<String>) -> Self {
        self.function_signature = Some(signature.into());
        self
    }

    /// Add type arguments.
    pub fn with_type_args(mut self, args: Vec<String>) -> Self {
        self.type_arguments = args;
        self
    }

    /// Add an input object.
    pub fn with_input_object(mut self, obj: ObjectSnapshot) -> Self {
        self.input_objects.push(obj);
        self
    }

    /// Set gas consumed before failure.
    pub fn with_gas_consumed(mut self, gas: u64) -> Self {
        self.gas_consumed_before_failure = gas;
        self
    }

    /// Set prior successful commands.
    pub fn with_prior_commands(mut self, commands: Vec<usize>) -> Self {
        self.prior_successful_commands = commands;
        self
    }

    /// Set coin operation context.
    pub fn with_coin_context(mut self, ctx: CoinOperationContext) -> Self {
        self.coin_balances = Some(ctx);
        self
    }

    /// Set abort information.
    pub fn with_abort_info(mut self, info: TransactionAbortInfo) -> Self {
        self.abort_info = Some(info);
        self
    }
}

impl ObjectSnapshot {
    /// Create a new object snapshot.
    pub fn new(
        id: impl Into<String>,
        type_tag: impl Into<String>,
        version: u64,
        data_size: usize,
        owner: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            type_tag: type_tag.into(),
            version,
            data_size,
            owner: owner.into(),
            modified_in_ptb: false,
        }
    }

    /// Mark this object as modified in the PTB.
    pub fn as_modified(mut self) -> Self {
        self.modified_in_ptb = true;
        self
    }

    /// Conditionally mark this object as modified.
    pub fn as_modified_if(mut self, modified: bool) -> Self {
        self.modified_in_ptb = modified;
        self
    }
}

impl fmt::Display for CommandErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Command #{} ({}) failed:",
            self.command_index, self.command_type
        )?;

        if let Some(ref sig) = self.function_signature {
            writeln!(f, "  Function: {}", sig)?;
        }

        if !self.type_arguments.is_empty() {
            writeln!(f, "  Type args: <{}>", self.type_arguments.join(", "))?;
        }

        if self.gas_consumed_before_failure > 0 {
            writeln!(f, "  Gas consumed: {}", self.gas_consumed_before_failure)?;
        }

        if !self.prior_successful_commands.is_empty() {
            writeln!(
                f,
                "  Prior successful commands: {:?}",
                self.prior_successful_commands
            )?;
        }

        if !self.input_objects.is_empty() {
            writeln!(f, "  Input objects:")?;
            for obj in &self.input_objects {
                write!(f, "    - {} ({})", obj.id, obj.type_tag)?;
                write!(f, " v{}", obj.version)?;
                write!(f, " [{} bytes]", obj.data_size)?;
                if obj.modified_in_ptb {
                    write!(f, " [modified]")?;
                }
                writeln!(f)?;
            }
        }

        if let Some(ref coin_ctx) = self.coin_balances {
            writeln!(f, "  Coin type: {}", coin_ctx.coin_type)?;
            if let Some(bal) = coin_ctx.source_balance {
                writeln!(f, "  Source balance: {}", bal)?;
            }
            if let Some(ref splits) = coin_ctx.requested_splits {
                writeln!(f, "  Requested splits: {:?}", splits)?;
            }
            if let Some(bal) = coin_ctx.destination_balance {
                writeln!(f, "  Destination balance: {}", bal)?;
            }
            if let Some(ref balances) = coin_ctx.source_balances {
                writeln!(f, "  Source balances to merge: {:?}", balances)?;
            }
        }

        if let Some(ref abort) = self.abort_info {
            writeln!(f, "  Abort in {}::{}", abort.module, abort.function)?;
            if let Some(ref const_name) = abort.constant_name {
                // CleverError - we have the actual constant name
                writeln!(f, "  Abort code: {} ({})", abort.abort_code, const_name)?;
            } else {
                writeln!(f, "  Abort code: {}", abort.abort_code)?;
            }
            if let Some(ref meaning) = abort.abort_meaning {
                writeln!(f, "  Meaning: {}", meaning)?;
            }
            if !abort.involved_objects.is_empty() {
                writeln!(
                    f,
                    "  Objects involved: {}",
                    abort.involved_objects.join(", ")
                )?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_context_builder() {
        let ctx = ExecutionContext::new()
            .at_command(3, "MoveCall 0x2::coin::split")
            .with_object(ObjectContext::new("0x123").with_type("0x2::coin::Coin<0x2::sui::SUI>"))
            .with_note("Testing context");

        assert_eq!(ctx.command_index, Some(3));
        assert_eq!(ctx.input_objects.len(), 1);
        assert_eq!(ctx.notes.len(), 1);
    }

    #[test]
    fn test_empty_context() {
        let ctx = ExecutionContext::new();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_context_display() {
        let ctx = ExecutionContext::new()
            .at_command(0, "MoveCall")
            .with_checkpoint(12345);

        let display = format!("{}", ctx);
        assert!(display.contains("Command: #0"));
        assert!(display.contains("Checkpoint: 12345"));
    }

    #[test]
    fn test_object_context_builder() {
        let obj = ObjectContext::new("0xabc")
            .with_type("0x2::coin::Coin<0x2::sui::SUI>")
            .with_version(42)
            .with_owner("0x123")
            .with_size(40);

        assert_eq!(obj.id, "0xabc");
        assert_eq!(
            obj.type_tag.as_deref(),
            Some("0x2::coin::Coin<0x2::sui::SUI>")
        );
        assert_eq!(obj.version, Some(42));
        assert_eq!(obj.owner.as_deref(), Some("0x123"));
        assert_eq!(obj.data_size, Some(40));
    }

    #[test]
    fn test_abort_code_context_generic() {
        assert!(get_abort_code_context(0, "0x1::test")
            .unwrap()
            .contains("Assertion"));
        assert!(get_abort_code_context(1, "0x1::test")
            .unwrap()
            .contains("overflow"));
    }

    #[test]
    fn test_abort_code_context_coin() {
        assert!(get_abort_code_context(1, "0x2::coin")
            .unwrap()
            .contains("balance"));
    }

    #[test]
    fn test_abort_code_context_pool() {
        let ctx = get_abort_code_context(4, "0x1234::pool");
        assert!(ctx.unwrap().contains("Slippage"));
    }

    #[test]
    fn test_abort_code_context_unknown() {
        assert!(get_abort_code_context(999, "0x1::unknown").is_none());
    }

    #[test]
    fn test_command_error_context_builder() {
        let obj = ObjectSnapshot::new(
            "0x123",
            "0x2::coin::Coin<0x2::sui::SUI>",
            42,
            40,
            "address:0xabc",
        );

        let ctx = CommandErrorContext::new(3, "MoveCall")
            .with_function("0x2::coin::split")
            .with_type_args(vec!["0x2::sui::SUI".into()])
            .with_input_object(obj)
            .with_gas_consumed(1000)
            .with_prior_commands(vec![0, 1, 2]);

        assert_eq!(ctx.command_index, 3);
        assert_eq!(ctx.command_type, "MoveCall");
        assert_eq!(ctx.function_signature.as_deref(), Some("0x2::coin::split"));
        assert_eq!(ctx.type_arguments.len(), 1);
        assert_eq!(ctx.input_objects.len(), 1);
        assert_eq!(ctx.gas_consumed_before_failure, 1000);
        assert_eq!(ctx.prior_successful_commands, vec![0, 1, 2]);
    }

    #[test]
    fn test_command_error_context_with_coin_operation() {
        let coin_ctx = CoinOperationContext {
            coin_type: "0x2::sui::SUI".into(),
            source_balance: Some(1000),
            requested_splits: Some(vec![300, 400, 500]),
            destination_balance: None,
            source_balances: None,
        };

        let ctx = CommandErrorContext::new(2, "SplitCoins").with_coin_context(coin_ctx);

        assert!(ctx.coin_balances.is_some());
        let coin = ctx.coin_balances.unwrap();
        assert_eq!(coin.source_balance, Some(1000));
        assert_eq!(coin.requested_splits, Some(vec![300, 400, 500]));
    }

    #[test]
    fn test_command_error_context_with_abort() {
        let abort = TransactionAbortInfo {
            module: "0x2::coin".into(),
            function: "split".into(),
            abort_code: 1,
            constant_name: Some("E_INSUFFICIENT_BALANCE".into()),
            abort_meaning: Some("Insufficient balance".into()),
            involved_objects: vec!["0x123".into()],
        };

        let ctx = CommandErrorContext::new(1, "MoveCall").with_abort_info(abort);

        assert!(ctx.abort_info.is_some());
        let info = ctx.abort_info.unwrap();
        assert_eq!(info.abort_code, 1);
        assert_eq!(info.abort_meaning.as_deref(), Some("Insufficient balance"));
    }

    #[test]
    fn test_command_error_context_display() {
        let obj = ObjectSnapshot::new("0x123", "Coin<SUI>", 42, 40, "shared").as_modified();

        let ctx = CommandErrorContext::new(3, "MoveCall")
            .with_function("0x2::coin::split")
            .with_input_object(obj)
            .with_gas_consumed(5000);

        let display = format!("{}", ctx);
        assert!(display.contains("Command #3 (MoveCall) failed"));
        assert!(display.contains("Function: 0x2::coin::split"));
        assert!(display.contains("Gas consumed: 5000"));
        assert!(display.contains("0x123"));
        assert!(display.contains("[modified]"));
    }

    #[test]
    fn test_object_snapshot_modified() {
        let obj = ObjectSnapshot::new("0x1", "Type", 1, 10, "owner").as_modified();
        assert!(obj.modified_in_ptb);
    }
}
