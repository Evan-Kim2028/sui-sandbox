//! Local disk cache for checkpoint data.
//!
//! This module provides a caching layer to avoid repeated network fetches during development.
//! The cache stores all checkpoint and object data needed to replay transactions.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Cache directory for storing fetched checkpoint data
const CACHE_DIR: &str = ".batch-ptb-cache";

/// Serializable checkpoint data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCheckpoint {
    pub sequence_number: u64,
    pub epoch: u64,
    pub transactions: Vec<CachedGrpcTransaction>,
}

/// Serializable transaction data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedGrpcTransaction {
    pub digest: String,
    pub sender: String,
    pub gas_budget: Option<u64>,
    pub gas_price: Option<u64>,
    pub checkpoint: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub epoch: Option<u64>,
    pub inputs: Vec<CachedGrpcInput>,
    pub commands: Vec<CachedGrpcCommand>,
    pub status: Option<String>,
    pub execution_error: Option<CachedExecutionError>,
    pub unchanged_loaded_runtime_objects: Vec<(String, u64)>,
    pub changed_objects: Vec<(String, u64)>,
    pub created_objects: Vec<(String, u64)>,
    pub unchanged_consensus_objects: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedExecutionError {
    pub description: Option<String>,
    pub command: Option<u64>,
    pub kind: Option<String>,
    pub move_abort: Option<CachedMoveAbort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMoveAbort {
    pub abort_code: u64,
    pub package: Option<String>,
    pub module: Option<String>,
    pub function_name: Option<String>,
    pub constant_name: Option<String>,
    pub rendered_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachedGrpcInput {
    Pure {
        bytes: Vec<u8>,
    },
    Object {
        object_id: String,
        version: u64,
        digest: String,
    },
    SharedObject {
        object_id: String,
        initial_version: u64,
        mutable: bool,
    },
    Receiving {
        object_id: String,
        version: u64,
        digest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachedGrpcCommand {
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<CachedGrpcArgument>,
    },
    TransferObjects {
        objects: Vec<CachedGrpcArgument>,
        address: CachedGrpcArgument,
    },
    SplitCoins {
        coin: CachedGrpcArgument,
        amounts: Vec<CachedGrpcArgument>,
    },
    MergeCoins {
        coin: CachedGrpcArgument,
        sources: Vec<CachedGrpcArgument>,
    },
    Publish {
        modules: Vec<Vec<u8>>,
        dependencies: Vec<String>,
    },
    MakeMoveVec {
        element_type: Option<String>,
        elements: Vec<CachedGrpcArgument>,
    },
    Upgrade {
        modules: Vec<Vec<u8>>,
        dependencies: Vec<String>,
        package: String,
        ticket: CachedGrpcArgument,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachedGrpcArgument {
    GasCoin,
    Input(u32),
    Result(u32),
    NestedResult(u32, u32),
}

/// Cached object data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedObject {
    pub object_id: String,
    pub version: u64,
    pub type_string: Option<String>,
    pub bcs: Option<Vec<u8>>,
    pub package_modules: Option<Vec<(String, Vec<u8>)>>,
    pub package_linkage: Option<Vec<CachedLinkage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedLinkage {
    pub original_id: String,
    pub upgraded_id: String,
}

/// Full cache for a checkpoint range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRangeCache {
    pub start_checkpoint: u64,
    pub end_checkpoint: u64,
    pub checkpoints: Vec<CachedCheckpoint>,
    /// Objects indexed by "object_id:version" for historical fetching
    pub objects: HashMap<String, CachedObject>,
    /// Dynamic field children: child_id -> (version, type_string, bcs)
    pub dynamic_field_children: HashMap<String, (u64, String, Vec<u8>)>,
}

#[allow(dead_code)]
impl CheckpointRangeCache {
    pub fn new(start: u64, end: u64) -> Self {
        Self {
            start_checkpoint: start,
            end_checkpoint: end,
            checkpoints: Vec::new(),
            objects: HashMap::new(),
            dynamic_field_children: HashMap::new(),
        }
    }

    /// Get the cache file path for a checkpoint range
    fn cache_path(start: u64, end: u64) -> std::path::PathBuf {
        Path::new(CACHE_DIR).join(format!("checkpoints_{}_{}.bincode", start, end))
    }

    /// Try to load from disk cache
    pub fn load(start: u64, end: u64) -> Option<Self> {
        let path = Self::cache_path(start, end);
        if !path.exists() {
            return None;
        }

        match fs::read(&path) {
            Ok(bytes) => match bincode::deserialize(&bytes) {
                Ok(cache) => {
                    println!("   Loaded cache from {:?}", path);
                    Some(cache)
                }
                Err(e) => {
                    eprintln!("   Failed to deserialize cache: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("   Failed to read cache: {}", e);
                None
            }
        }
    }

    /// Save to disk cache
    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(CACHE_DIR)?;
        let path = Self::cache_path(self.start_checkpoint, self.end_checkpoint);
        let bytes = bincode::serialize(self)?;
        let bytes_len = bytes.len();
        fs::write(&path, bytes)?;
        println!("   Saved cache to {:?} ({} bytes)", path, bytes_len);
        Ok(())
    }

    /// Add an object to the cache
    pub fn add_object(&mut self, obj: CachedObject) {
        let key = format!("{}:{}", obj.object_id, obj.version);
        self.objects.insert(key, obj);
    }

    /// Get an object by ID and version
    pub fn get_object(&self, object_id: &str, version: u64) -> Option<&CachedObject> {
        let key = format!("{}:{}", object_id, version);
        self.objects.get(&key)
    }

    /// Get an object by ID (any version - returns the first match)
    pub fn get_object_any_version(&self, object_id: &str) -> Option<&CachedObject> {
        // Normalize the object_id
        let normalized = sui_sandbox_core::utilities::normalize_address(object_id);
        for (key, obj) in &self.objects {
            if key.starts_with(&normalized) || key.starts_with(object_id) {
                return Some(obj);
            }
        }
        None
    }
}

// Conversion functions from GrpcClient types to cached types
use sui_data_fetcher::grpc::{
    GrpcArgument, GrpcCheckpoint, GrpcCommand, GrpcExecutionError, GrpcInput, GrpcMoveAbort,
    GrpcObject, GrpcTransaction,
};

impl From<&GrpcCheckpoint> for CachedCheckpoint {
    fn from(cp: &GrpcCheckpoint) -> Self {
        Self {
            sequence_number: cp.sequence_number,
            epoch: cp.epoch,
            transactions: cp
                .transactions
                .iter()
                .map(CachedGrpcTransaction::from)
                .collect(),
        }
    }
}

impl From<&GrpcTransaction> for CachedGrpcTransaction {
    fn from(tx: &GrpcTransaction) -> Self {
        Self {
            digest: tx.digest.clone(),
            sender: tx.sender.clone(),
            gas_budget: tx.gas_budget,
            gas_price: tx.gas_price,
            checkpoint: tx.checkpoint,
            timestamp_ms: tx.timestamp_ms,
            epoch: tx.epoch,
            inputs: tx.inputs.iter().map(CachedGrpcInput::from).collect(),
            commands: tx.commands.iter().map(CachedGrpcCommand::from).collect(),
            status: tx.status.clone(),
            execution_error: tx.execution_error.as_ref().map(CachedExecutionError::from),
            unchanged_loaded_runtime_objects: tx.unchanged_loaded_runtime_objects.clone(),
            changed_objects: tx.changed_objects.clone(),
            created_objects: tx.created_objects.clone(),
            unchanged_consensus_objects: tx.unchanged_consensus_objects.clone(),
        }
    }
}

impl From<&GrpcExecutionError> for CachedExecutionError {
    fn from(e: &GrpcExecutionError) -> Self {
        Self {
            description: e.description.clone(),
            command: e.command,
            kind: e.kind.clone(),
            move_abort: e.move_abort.as_ref().map(CachedMoveAbort::from),
        }
    }
}

impl From<&GrpcMoveAbort> for CachedMoveAbort {
    fn from(a: &GrpcMoveAbort) -> Self {
        Self {
            abort_code: a.abort_code,
            package: a.package.clone(),
            module: a.module.clone(),
            function_name: a.function_name.clone(),
            constant_name: a.constant_name.clone(),
            rendered_message: a.rendered_message.clone(),
        }
    }
}

impl From<&GrpcInput> for CachedGrpcInput {
    fn from(input: &GrpcInput) -> Self {
        match input {
            GrpcInput::Pure { bytes } => CachedGrpcInput::Pure {
                bytes: bytes.clone(),
            },
            GrpcInput::Object {
                object_id,
                version,
                digest,
            } => CachedGrpcInput::Object {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                mutable,
            } => CachedGrpcInput::SharedObject {
                object_id: object_id.clone(),
                initial_version: *initial_version,
                mutable: *mutable,
            },
            GrpcInput::Receiving {
                object_id,
                version,
                digest,
            } => CachedGrpcInput::Receiving {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
        }
    }
}

impl From<&GrpcCommand> for CachedGrpcCommand {
    fn from(cmd: &GrpcCommand) -> Self {
        match cmd {
            GrpcCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => CachedGrpcCommand::MoveCall {
                package: package.clone(),
                module: module.clone(),
                function: function.clone(),
                type_arguments: type_arguments.clone(),
                arguments: arguments.iter().map(CachedGrpcArgument::from).collect(),
            },
            GrpcCommand::TransferObjects { objects, address } => {
                CachedGrpcCommand::TransferObjects {
                    objects: objects.iter().map(CachedGrpcArgument::from).collect(),
                    address: CachedGrpcArgument::from(address),
                }
            }
            GrpcCommand::SplitCoins { coin, amounts } => CachedGrpcCommand::SplitCoins {
                coin: CachedGrpcArgument::from(coin),
                amounts: amounts.iter().map(CachedGrpcArgument::from).collect(),
            },
            GrpcCommand::MergeCoins { coin, sources } => CachedGrpcCommand::MergeCoins {
                coin: CachedGrpcArgument::from(coin),
                sources: sources.iter().map(CachedGrpcArgument::from).collect(),
            },
            GrpcCommand::Publish {
                modules,
                dependencies,
            } => CachedGrpcCommand::Publish {
                modules: modules.clone(),
                dependencies: dependencies.clone(),
            },
            GrpcCommand::MakeMoveVec {
                element_type,
                elements,
            } => CachedGrpcCommand::MakeMoveVec {
                element_type: element_type.clone(),
                elements: elements.iter().map(CachedGrpcArgument::from).collect(),
            },
            GrpcCommand::Upgrade {
                modules,
                dependencies,
                package,
                ticket,
            } => CachedGrpcCommand::Upgrade {
                modules: modules.clone(),
                dependencies: dependencies.clone(),
                package: package.clone(),
                ticket: CachedGrpcArgument::from(ticket),
            },
        }
    }
}

impl From<&GrpcArgument> for CachedGrpcArgument {
    fn from(arg: &GrpcArgument) -> Self {
        match arg {
            GrpcArgument::GasCoin => CachedGrpcArgument::GasCoin,
            GrpcArgument::Input(i) => CachedGrpcArgument::Input(*i),
            GrpcArgument::Result(i) => CachedGrpcArgument::Result(*i),
            GrpcArgument::NestedResult(i, j) => CachedGrpcArgument::NestedResult(*i, *j),
        }
    }
}

impl From<&GrpcObject> for CachedObject {
    fn from(obj: &GrpcObject) -> Self {
        Self {
            object_id: obj.object_id.clone(),
            version: obj.version,
            type_string: obj.type_string.clone(),
            bcs: obj.bcs.clone(),
            package_modules: obj.package_modules.clone(),
            package_linkage: obj.package_linkage.as_ref().map(|linkages| {
                linkages
                    .iter()
                    .map(|l| CachedLinkage {
                        original_id: l.original_id.clone(),
                        upgraded_id: l.upgraded_id.clone(),
                    })
                    .collect()
            }),
        }
    }
}

// Conversion back to GrpcTransaction for replay
impl CachedGrpcTransaction {
    pub fn to_grpc(&self) -> GrpcTransaction {
        GrpcTransaction {
            digest: self.digest.clone(),
            sender: self.sender.clone(),
            gas_budget: self.gas_budget,
            gas_price: self.gas_price,
            checkpoint: self.checkpoint,
            timestamp_ms: self.timestamp_ms,
            epoch: self.epoch,
            inputs: self.inputs.iter().map(|i| i.to_grpc()).collect(),
            commands: self.commands.iter().map(|c| c.to_grpc()).collect(),
            status: self.status.clone(),
            execution_error: self.execution_error.as_ref().map(|e| e.to_grpc()),
            unchanged_loaded_runtime_objects: self.unchanged_loaded_runtime_objects.clone(),
            changed_objects: self.changed_objects.clone(),
            created_objects: self.created_objects.clone(),
            unchanged_consensus_objects: self.unchanged_consensus_objects.clone(),
        }
    }
}

impl CachedExecutionError {
    pub fn to_grpc(&self) -> GrpcExecutionError {
        GrpcExecutionError {
            description: self.description.clone(),
            command: self.command,
            kind: self.kind.clone(),
            move_abort: self.move_abort.as_ref().map(|a| a.to_grpc()),
        }
    }
}

impl CachedMoveAbort {
    pub fn to_grpc(&self) -> GrpcMoveAbort {
        GrpcMoveAbort {
            abort_code: self.abort_code,
            package: self.package.clone(),
            module: self.module.clone(),
            function_name: self.function_name.clone(),
            constant_name: self.constant_name.clone(),
            rendered_message: self.rendered_message.clone(),
        }
    }
}

impl CachedGrpcInput {
    pub fn to_grpc(&self) -> GrpcInput {
        match self {
            CachedGrpcInput::Pure { bytes } => GrpcInput::Pure {
                bytes: bytes.clone(),
            },
            CachedGrpcInput::Object {
                object_id,
                version,
                digest,
            } => GrpcInput::Object {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
            CachedGrpcInput::SharedObject {
                object_id,
                initial_version,
                mutable,
            } => GrpcInput::SharedObject {
                object_id: object_id.clone(),
                initial_version: *initial_version,
                mutable: *mutable,
            },
            CachedGrpcInput::Receiving {
                object_id,
                version,
                digest,
            } => GrpcInput::Receiving {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
        }
    }
}

impl CachedGrpcCommand {
    pub fn to_grpc(&self) -> GrpcCommand {
        match self {
            CachedGrpcCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => GrpcCommand::MoveCall {
                package: package.clone(),
                module: module.clone(),
                function: function.clone(),
                type_arguments: type_arguments.clone(),
                arguments: arguments.iter().map(|a| a.to_grpc()).collect(),
            },
            CachedGrpcCommand::TransferObjects { objects, address } => {
                GrpcCommand::TransferObjects {
                    objects: objects.iter().map(|o| o.to_grpc()).collect(),
                    address: address.to_grpc(),
                }
            }
            CachedGrpcCommand::SplitCoins { coin, amounts } => GrpcCommand::SplitCoins {
                coin: coin.to_grpc(),
                amounts: amounts.iter().map(|a| a.to_grpc()).collect(),
            },
            CachedGrpcCommand::MergeCoins { coin, sources } => GrpcCommand::MergeCoins {
                coin: coin.to_grpc(),
                sources: sources.iter().map(|s| s.to_grpc()).collect(),
            },
            CachedGrpcCommand::Publish {
                modules,
                dependencies,
            } => GrpcCommand::Publish {
                modules: modules.clone(),
                dependencies: dependencies.clone(),
            },
            CachedGrpcCommand::MakeMoveVec {
                element_type,
                elements,
            } => GrpcCommand::MakeMoveVec {
                element_type: element_type.clone(),
                elements: elements.iter().map(|e| e.to_grpc()).collect(),
            },
            CachedGrpcCommand::Upgrade {
                modules,
                dependencies,
                package,
                ticket,
            } => GrpcCommand::Upgrade {
                modules: modules.clone(),
                dependencies: dependencies.clone(),
                package: package.clone(),
                ticket: ticket.to_grpc(),
            },
        }
    }
}

impl CachedGrpcArgument {
    pub fn to_grpc(&self) -> GrpcArgument {
        match self {
            CachedGrpcArgument::GasCoin => GrpcArgument::GasCoin,
            CachedGrpcArgument::Input(i) => GrpcArgument::Input(*i),
            CachedGrpcArgument::Result(i) => GrpcArgument::Result(*i),
            CachedGrpcArgument::NestedResult(i, j) => GrpcArgument::NestedResult(*i, *j),
        }
    }
}
