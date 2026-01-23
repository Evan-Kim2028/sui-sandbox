//! Simulation error types matching Sui mainnet error semantics.
//!
//! This module provides structured error types that map to Sui execution errors,
//! enabling programmatic error handling and meaningful feedback.

use move_core_types::account_address::AccountAddress;

/// Structured error types matching Sui mainnet error semantics.
#[derive(Debug, Clone)]
pub enum SimulationError {
    /// LINKER_ERROR: A required package/module is not available.
    MissingPackage {
        /// The address of the missing package
        address: String,
        /// The specific module within the package (if known)
        module: Option<String>,
        /// Packages that depend on this one (helps trace the dependency)
        #[allow(dead_code)]
        referenced_by: Option<Vec<String>>,
        /// Whether this package has known upgrades
        upgrade_info: Option<crate::error_context::PackageUpgradeInfo>,
    },

    /// A required object is not available.
    MissingObject {
        /// Object ID that was not found
        id: String,
        /// Expected type of the object
        expected_type: Option<String>,
        /// The command that tried to access this object
        command_index: Option<usize>,
        /// Version requested (if historical)
        requested_version: Option<u64>,
    },

    /// Type mismatch between expected and provided.
    TypeMismatch {
        /// Expected type
        expected: String,
        /// Actual type provided
        got: String,
        /// Where the mismatch occurred (e.g., "input 0", "command 3 argument 1")
        location: String,
        /// The command index where this occurred
        command_index: Option<usize>,
    },

    /// ABORTED: Contract assertion failed.
    ContractAbort {
        /// Full module path (e.g., "0x1eabed72::pool")
        module: String,
        /// Function name
        function: String,
        /// Abort code from the contract
        abort_code: u64,
        /// Human-readable message if available
        message: Option<String>,
        /// The command index that aborted
        command_index: Option<usize>,
        /// What objects were involved in this call
        involved_objects: Option<Vec<String>>,
    },

    /// FAILED_TO_DESERIALIZE_ARGUMENT: Deserialization failed for an argument.
    DeserializationFailed {
        /// Index of the argument that failed
        argument_index: usize,
        /// Expected type for the argument
        expected_type: String,
        /// The command index where this occurred
        command_index: Option<usize>,
        /// Size of the provided data
        data_size: Option<usize>,
    },

    /// Other execution error.
    ExecutionError {
        /// Error message
        message: String,
        /// The command index where this occurred
        command_index: Option<usize>,
    },

    /// Shared object lock conflict (concurrent access detection).
    SharedObjectLockConflict {
        /// Object ID that had the conflict
        object_id: AccountAddress,
        /// Who/what holds the lock
        held_by: Option<String>,
        /// Reason for the conflict
        reason: String,
        /// The command index where this occurred
        command_index: Option<usize>,
    },
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulationError::MissingPackage {
                address,
                module,
                referenced_by: _,
                upgrade_info,
            } => {
                if let Some(m) = module {
                    write!(
                        f,
                        "LINKER_ERROR: Cannot find ModuleId {{ address: {}, name: Identifier(\"{}\") }}",
                        address.trim_start_matches("0x"),
                        m
                    )?;
                } else {
                    write!(f, "LINKER_ERROR: Cannot find package {}", address)?;
                }
                if let Some(info) = upgrade_info {
                    write!(
                        f,
                        " (upgraded from {} to {} at version {})",
                        info.original_id, info.storage_id, info.version
                    )?;
                }
                Ok(())
            }
            SimulationError::MissingObject {
                id,
                expected_type,
                command_index,
                requested_version,
            } => {
                write!(f, "ObjectNotFound: {}", id)?;
                if let Some(t) = expected_type {
                    write!(f, " (expected type: {})", t)?;
                }
                if let Some(idx) = command_index {
                    write!(f, " at command #{}", idx)?;
                }
                if let Some(v) = requested_version {
                    write!(f, " version {}", v)?;
                }
                Ok(())
            }
            SimulationError::TypeMismatch {
                expected,
                got,
                location,
                command_index,
            } => {
                write!(
                    f,
                    "TYPE_MISMATCH at {}: expected {}, got {}",
                    location, expected, got
                )?;
                if let Some(idx) = command_index {
                    write!(f, " (command #{})", idx)?;
                }
                Ok(())
            }
            SimulationError::ContractAbort {
                module,
                function,
                abort_code,
                message,
                command_index,
                involved_objects,
            } => {
                write!(
                    f,
                    "MoveAbort(MoveLocation {{ module: {}::{}, function: 0, instruction: 0 }}, {})",
                    module, function, abort_code
                )?;
                if let Some(msg) = message {
                    write!(f, " in {}", msg)?;
                }
                if let Some(idx) = command_index {
                    write!(f, " at command #{}", idx)?;
                }
                // Add abort code context
                if let Some(context) =
                    crate::error_context::get_abort_code_context(*abort_code, module)
                {
                    write!(f, " ({})", context)?;
                }
                if let Some(objs) = involved_objects {
                    if !objs.is_empty() {
                        write!(f, " [objects: {}]", objs.join(", "))?;
                    }
                }
                Ok(())
            }
            SimulationError::DeserializationFailed {
                argument_index,
                expected_type,
                command_index,
                data_size,
            } => {
                write!(
                    f,
                    "FAILED_TO_DESERIALIZE_ARGUMENT: argument {} cannot be deserialized as {}",
                    argument_index, expected_type
                )?;
                if let Some(idx) = command_index {
                    write!(f, " at command #{}", idx)?;
                }
                if let Some(size) = data_size {
                    write!(f, " (data size: {} bytes)", size)?;
                }
                Ok(())
            }
            SimulationError::ExecutionError {
                message,
                command_index,
            } => {
                write!(f, "{}", message)?;
                if let Some(idx) = command_index {
                    write!(f, " at command #{}", idx)?;
                }
                Ok(())
            }
            SimulationError::SharedObjectLockConflict {
                object_id,
                held_by,
                reason,
                command_index,
            } => {
                if let Some(tx) = held_by {
                    write!(
                        f,
                        "SharedObjectLockConflict: object {} is locked by transaction {}: {}",
                        object_id, tx, reason
                    )?;
                } else {
                    write!(
                        f,
                        "SharedObjectLockConflict: object {} lock conflict: {}",
                        object_id, reason
                    )?;
                }
                if let Some(idx) = command_index {
                    write!(f, " at command #{}", idx)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SimulationError {}
