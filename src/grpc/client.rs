//! gRPC Client implementation for Sui
//!
//! Sui's public fullnodes now support gRPC:
//! - **Mainnet**: `https://fullnode.mainnet.sui.io:443` (streaming + queries)
//! - **Archive**: `https://archive.mainnet.sui.io:443` (historical queries, no streaming)
//! - **Testnet**: `https://fullnode.testnet.sui.io:443`
//!
//! Set the SUI_GRPC_ENDPOINT environment variable or use `GrpcClient::new()`.

use anyhow::{anyhow, Result};
use futures::StreamExt;
use tonic::transport::Channel;

use super::generated::sui_rpc_v2::{
    self as proto, ledger_service_client::LedgerServiceClient,
    subscription_service_client::SubscriptionServiceClient,
};

/// gRPC client for Sui network.
///
/// Provides streaming subscriptions and batch fetching capabilities.
///
/// # Public Endpoints
///
/// Sui's public fullnodes support gRPC:
/// - `https://fullnode.mainnet.sui.io:443` - Live streaming + queries
/// - `https://archive.mainnet.sui.io:443` - Historical queries (no streaming)
/// - `https://fullnode.testnet.sui.io:443` - Testnet
pub struct GrpcClient {
    endpoint: String,
    channel: Channel,
}

impl GrpcClient {
    /// Create a client for Sui mainnet.
    ///
    /// Reads the `SUI_GRPC_ENDPOINT` environment variable, or defaults to
    /// `https://fullnode.mainnet.sui.io:443`.
    pub async fn mainnet() -> Result<Self> {
        let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
            .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
        Self::new(&endpoint).await
    }

    /// Create a client for Sui testnet.
    ///
    /// Reads the `SUI_GRPC_TESTNET_ENDPOINT` environment variable, or defaults to
    /// `https://fullnode.testnet.sui.io:443`.
    pub async fn testnet() -> Result<Self> {
        let endpoint = std::env::var("SUI_GRPC_TESTNET_ENDPOINT")
            .unwrap_or_else(|_| "https://fullnode.testnet.sui.io:443".to_string());
        Self::new(&endpoint).await
    }

    /// Create a client for Sui mainnet archive (historical data).
    ///
    /// The archive has full history from checkpoint 0 but doesn't support streaming.
    /// Use for historical queries only.
    pub async fn archive() -> Result<Self> {
        let endpoint = std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT")
            .unwrap_or_else(|_| "https://archive.mainnet.sui.io:443".to_string());
        Self::new(&endpoint).await
    }

    /// Create a client with a custom endpoint.
    pub async fn new(endpoint: &str) -> Result<Self> {
        // Configure TLS for HTTPS endpoints
        let channel = if endpoint.starts_with("https://") {
            Channel::from_shared(endpoint.to_string())?
                .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())?
                .connect()
                .await
                .map_err(|e| anyhow!("Failed to connect to gRPC endpoint {}: {}", endpoint, e))?
        } else {
            Channel::from_shared(endpoint.to_string())?
                .connect()
                .await
                .map_err(|e| anyhow!("Failed to connect to gRPC endpoint {}: {}", endpoint, e))?
        };

        Ok(Self {
            endpoint: endpoint.to_string(),
            channel,
        })
    }

    /// Get the endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    // =========================================================================
    // Service Info
    // =========================================================================

    /// Get service info (chain ID, current epoch, checkpoint height).
    pub async fn get_service_info(&self) -> Result<ServiceInfo> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let response = client
            .get_service_info(proto::GetServiceInfoRequest {})
            .await
            .map_err(|e| anyhow!("gRPC error: {}", e))?;

        let info = response.into_inner();

        Ok(ServiceInfo {
            chain_id: info.chain_id.unwrap_or_default(),
            chain: info.chain.unwrap_or_default(),
            epoch: info.epoch.unwrap_or(0),
            checkpoint_height: info.checkpoint_height.unwrap_or(0),
            lowest_available_checkpoint: info.lowest_available_checkpoint.unwrap_or(0),
        })
    }

    // =========================================================================
    // Subscriptions (Streaming)
    // =========================================================================

    /// Subscribe to new checkpoints as they're finalized.
    ///
    /// Returns a stream of checkpoints with full transaction data.
    pub async fn subscribe_checkpoints(&self) -> Result<CheckpointStream> {
        let mut client = SubscriptionServiceClient::new(self.channel.clone());

        // Request full checkpoint data including transactions
        let request = proto::SubscribeCheckpointsRequest {
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()], // Request all fields
            }),
        };

        let response = client
            .subscribe_checkpoints(request)
            .await
            .map_err(|e| anyhow!("gRPC subscription error: {}", e))?;

        Ok(CheckpointStream {
            inner: Box::pin(response.into_inner()),
        })
    }

    // =========================================================================
    // Object Fetching
    // =========================================================================

    /// Fetch a single object by ID.
    pub async fn get_object(&self, object_id: &str) -> Result<Option<GrpcObject>> {
        self.get_object_at_version(object_id, None).await
    }

    /// Fetch an object at a specific version (for historical replay).
    ///
    /// If version is None, returns the latest available version.
    /// This is useful for fetching historical object state from the archive.
    pub async fn get_object_at_version(
        &self,
        object_id: &str,
        version: Option<u64>,
    ) -> Result<Option<GrpcObject>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::GetObjectRequest {
            object_id: Some(object_id.to_string()),
            version,
            read_mask: Some(prost_types::FieldMask {
                paths: vec![
                    "object_id".to_string(),
                    "version".to_string(),
                    "digest".to_string(),
                    "object_type".to_string(),
                    "owner".to_string(),
                    "bcs".to_string(),      // Full object BCS (includes type tag)
                    "contents".to_string(), // Move struct BCS (starts with UID)
                ],
            }),
        };

        let response = client
            .get_object(request)
            .await
            .map_err(|e| anyhow!("gRPC error fetching object: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.object.map(GrpcObject::from_proto))
    }

    /// Batch fetch multiple objects.
    pub async fn batch_get_objects(&self, object_ids: &[&str]) -> Result<Vec<Option<GrpcObject>>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let requests: Vec<proto::GetObjectRequest> = object_ids
            .iter()
            .map(|id| proto::GetObjectRequest {
                object_id: Some(id.to_string()),
                version: None,
                read_mask: None,
            })
            .collect();

        let request = proto::BatchGetObjectsRequest {
            requests,
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
        };

        let response = client
            .batch_get_objects(request)
            .await
            .map_err(|e| anyhow!("gRPC batch error: {}", e))?;

        let results = response
            .into_inner()
            .objects
            .into_iter()
            .map(|r| match r.result {
                Some(proto::get_object_result::Result::Object(obj)) => {
                    Some(GrpcObject::from_proto(obj))
                }
                _ => None,
            })
            .collect();

        Ok(results)
    }

    // =========================================================================
    // Transaction Fetching
    // =========================================================================

    /// Fetch a single transaction by digest.
    pub async fn get_transaction(&self, digest: &str) -> Result<Option<GrpcTransaction>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::GetTransactionRequest {
            digest: Some(digest.to_string()),
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
        };

        let response = client
            .get_transaction(request)
            .await
            .map_err(|e| anyhow!("gRPC error fetching transaction: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.transaction.map(GrpcTransaction::from_proto))
    }

    /// Batch fetch multiple transactions.
    pub async fn batch_get_transactions(
        &self,
        digests: &[&str],
    ) -> Result<Vec<Option<GrpcTransaction>>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::BatchGetTransactionsRequest {
            digests: digests.iter().map(|s| s.to_string()).collect(),
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
        };

        let response = client
            .batch_get_transactions(request)
            .await
            .map_err(|e| anyhow!("gRPC batch error: {}", e))?;

        let results = response
            .into_inner()
            .transactions
            .into_iter()
            .map(|r| match r.result {
                Some(proto::get_transaction_result::Result::Transaction(tx)) => {
                    Some(GrpcTransaction::from_proto(tx))
                }
                _ => None,
            })
            .collect();

        Ok(results)
    }

    // =========================================================================
    // Checkpoint Fetching
    // =========================================================================

    /// Fetch a checkpoint by sequence number.
    pub async fn get_checkpoint(&self, sequence_number: u64) -> Result<Option<GrpcCheckpoint>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::GetCheckpointRequest {
            checkpoint_id: Some(proto::get_checkpoint_request::CheckpointId::SequenceNumber(
                sequence_number,
            )),
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
        };

        let response = client
            .get_checkpoint(request)
            .await
            .map_err(|e| anyhow!("gRPC error fetching checkpoint: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.checkpoint.map(GrpcCheckpoint::from_proto))
    }

    /// Fetch the latest checkpoint.
    pub async fn get_latest_checkpoint(&self) -> Result<Option<GrpcCheckpoint>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::GetCheckpointRequest {
            checkpoint_id: None, // None = latest
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
        };

        let response = client
            .get_checkpoint(request)
            .await
            .map_err(|e| anyhow!("gRPC error fetching latest checkpoint: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.checkpoint.map(GrpcCheckpoint::from_proto))
    }
}

// =============================================================================
// Data Types
// =============================================================================

/// Service information from gRPC endpoint.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub chain_id: String,
    pub chain: String,
    pub epoch: u64,
    pub checkpoint_height: u64,
    pub lowest_available_checkpoint: u64,
}

/// Checkpoint stream from subscription.
pub struct CheckpointStream {
    inner: std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<proto::SubscribeCheckpointsResponse, tonic::Status>>
                + Send,
        >,
    >,
}

impl CheckpointStream {
    /// Get the next checkpoint from the stream.
    pub async fn next(&mut self) -> Option<Result<GrpcCheckpoint>> {
        match self.inner.next().await {
            Some(Ok(response)) => {
                let checkpoint = response.checkpoint.map(GrpcCheckpoint::from_proto)?;
                Some(Ok(checkpoint))
            }
            Some(Err(e)) => Some(Err(anyhow!("Stream error: {}", e))),
            None => None,
        }
    }
}

/// A checkpoint with full transaction data.
#[derive(Debug, Clone)]
pub struct GrpcCheckpoint {
    pub sequence_number: u64,
    pub digest: String,
    pub timestamp_ms: Option<u64>,
    pub transactions: Vec<GrpcTransaction>,
}

impl GrpcCheckpoint {
    fn from_proto(proto: proto::Checkpoint) -> Self {
        let timestamp_ms = proto
            .summary
            .as_ref()
            .and_then(|s| s.timestamp.as_ref())
            .map(|t| (t.seconds as u64) * 1000 + (t.nanos as u64) / 1_000_000);

        Self {
            sequence_number: proto.sequence_number.unwrap_or(0),
            digest: proto.digest.unwrap_or_default(),
            timestamp_ms,
            transactions: proto
                .transactions
                .into_iter()
                .map(GrpcTransaction::from_proto)
                .collect(),
        }
    }
}

/// A transaction with full PTB data.
#[derive(Debug, Clone)]
pub struct GrpcTransaction {
    pub digest: String,
    pub sender: String,
    pub gas_budget: Option<u64>,
    pub gas_price: Option<u64>,
    pub checkpoint: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub inputs: Vec<GrpcInput>,
    pub commands: Vec<GrpcCommand>,
    pub status: Option<String>,
}

impl GrpcTransaction {
    fn from_proto(proto: proto::ExecutedTransaction) -> Self {
        let tx = proto.transaction.as_ref();
        let effects = proto.effects.as_ref();

        let (inputs, commands) = tx
            .and_then(|t| t.kind.as_ref())
            .and_then(|k| k.data.as_ref())
            .map(|data| match data {
                proto::transaction_kind::Data::ProgrammableTransaction(ptb) => {
                    let inputs: Vec<GrpcInput> =
                        ptb.inputs.iter().map(GrpcInput::from_proto).collect();
                    let commands: Vec<GrpcCommand> =
                        ptb.commands.iter().map(GrpcCommand::from_proto).collect();
                    (inputs, commands)
                }
                _ => (vec![], vec![]),
            })
            .unwrap_or((vec![], vec![]));

        let gas_payment = tx.and_then(|t| t.gas_payment.as_ref());

        let timestamp_ms = proto
            .timestamp
            .as_ref()
            .map(|t| (t.seconds as u64) * 1000 + (t.nanos as u64) / 1_000_000);

        let status = effects.and_then(|e| e.status.as_ref()).map(|s| {
            if s.success.unwrap_or(false) {
                "success".to_string()
            } else {
                "failure".to_string()
            }
        });

        Self {
            digest: proto.digest.unwrap_or_default(),
            sender: tx.and_then(|t| t.sender.clone()).unwrap_or_default(),
            gas_budget: gas_payment.and_then(|g| g.budget),
            gas_price: gas_payment.and_then(|g| g.price),
            checkpoint: proto.checkpoint,
            timestamp_ms,
            inputs,
            commands,
            status,
        }
    }

    /// Check if this is a programmable transaction (not a system tx).
    pub fn is_ptb(&self) -> bool {
        !self.sender.is_empty() && (!self.commands.is_empty() || self.gas_budget.unwrap_or(0) > 0)
    }
}

/// An input to a PTB.
#[derive(Debug, Clone)]
pub enum GrpcInput {
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

impl GrpcInput {
    fn from_proto(proto: &proto::Input) -> Self {
        use proto::input::InputKind;

        match InputKind::try_from(proto.kind.unwrap_or(0)) {
            Ok(InputKind::Pure) => GrpcInput::Pure {
                bytes: proto.pure.clone().unwrap_or_default(),
            },
            Ok(InputKind::ImmutableOrOwned) => GrpcInput::Object {
                object_id: proto.object_id.clone().unwrap_or_default(),
                version: proto.version.unwrap_or(0),
                digest: proto.digest.clone().unwrap_or_default(),
            },
            Ok(InputKind::Shared) => GrpcInput::SharedObject {
                object_id: proto.object_id.clone().unwrap_or_default(),
                initial_version: proto.version.unwrap_or(0),
                mutable: proto.mutable.unwrap_or(false),
            },
            Ok(InputKind::Receiving) => GrpcInput::Receiving {
                object_id: proto.object_id.clone().unwrap_or_default(),
                version: proto.version.unwrap_or(0),
                digest: proto.digest.clone().unwrap_or_default(),
            },
            _ => GrpcInput::Pure { bytes: vec![] },
        }
    }
}

/// A PTB command.
#[derive(Debug, Clone)]
pub enum GrpcCommand {
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<GrpcArgument>,
    },
    TransferObjects {
        objects: Vec<GrpcArgument>,
        address: GrpcArgument,
    },
    SplitCoins {
        coin: GrpcArgument,
        amounts: Vec<GrpcArgument>,
    },
    MergeCoins {
        coin: GrpcArgument,
        sources: Vec<GrpcArgument>,
    },
    Publish {
        modules: Vec<Vec<u8>>,
        dependencies: Vec<String>,
    },
    MakeMoveVec {
        element_type: Option<String>,
        elements: Vec<GrpcArgument>,
    },
    Upgrade {
        modules: Vec<Vec<u8>>,
        dependencies: Vec<String>,
        package: String,
        ticket: GrpcArgument,
    },
}

impl GrpcCommand {
    fn from_proto(proto: &proto::Command) -> Self {
        match &proto.command {
            Some(proto::command::Command::MoveCall(mc)) => GrpcCommand::MoveCall {
                package: mc.package.clone().unwrap_or_default(),
                module: mc.module.clone().unwrap_or_default(),
                function: mc.function.clone().unwrap_or_default(),
                type_arguments: mc.type_arguments.clone(),
                arguments: mc.arguments.iter().map(GrpcArgument::from_proto).collect(),
            },
            Some(proto::command::Command::TransferObjects(to)) => GrpcCommand::TransferObjects {
                objects: to.objects.iter().map(GrpcArgument::from_proto).collect(),
                address: to
                    .address
                    .as_ref()
                    .map(GrpcArgument::from_proto)
                    .unwrap_or(GrpcArgument::GasCoin),
            },
            Some(proto::command::Command::SplitCoins(sc)) => GrpcCommand::SplitCoins {
                coin: sc
                    .coin
                    .as_ref()
                    .map(GrpcArgument::from_proto)
                    .unwrap_or(GrpcArgument::GasCoin),
                amounts: sc.amounts.iter().map(GrpcArgument::from_proto).collect(),
            },
            Some(proto::command::Command::MergeCoins(mc)) => GrpcCommand::MergeCoins {
                coin: mc
                    .coin
                    .as_ref()
                    .map(GrpcArgument::from_proto)
                    .unwrap_or(GrpcArgument::GasCoin),
                sources: mc
                    .coins_to_merge
                    .iter()
                    .map(GrpcArgument::from_proto)
                    .collect(),
            },
            Some(proto::command::Command::Publish(p)) => GrpcCommand::Publish {
                modules: p.modules.clone(),
                dependencies: p.dependencies.clone(),
            },
            Some(proto::command::Command::MakeMoveVector(mmv)) => GrpcCommand::MakeMoveVec {
                element_type: mmv.element_type.clone(),
                elements: mmv.elements.iter().map(GrpcArgument::from_proto).collect(),
            },
            Some(proto::command::Command::Upgrade(u)) => GrpcCommand::Upgrade {
                modules: u.modules.clone(),
                dependencies: u.dependencies.clone(),
                package: u.package.clone().unwrap_or_default(),
                ticket: u
                    .ticket
                    .as_ref()
                    .map(GrpcArgument::from_proto)
                    .unwrap_or(GrpcArgument::GasCoin),
            },
            None => GrpcCommand::MoveCall {
                package: String::new(),
                module: String::new(),
                function: String::new(),
                type_arguments: vec![],
                arguments: vec![],
            },
        }
    }
}

/// An argument to a PTB command.
#[derive(Debug, Clone)]
pub enum GrpcArgument {
    GasCoin,
    Input(u32),
    Result(u32),
    NestedResult(u32, u32),
}

impl GrpcArgument {
    fn from_proto(proto: &proto::Argument) -> Self {
        use proto::argument::ArgumentKind;

        match ArgumentKind::try_from(proto.kind.unwrap_or(0)) {
            Ok(ArgumentKind::Gas) => GrpcArgument::GasCoin,
            Ok(ArgumentKind::Input) => GrpcArgument::Input(proto.input.unwrap_or(0)),
            Ok(ArgumentKind::Result) => {
                if let Some(sub) = proto.subresult {
                    GrpcArgument::NestedResult(proto.result.unwrap_or(0), sub)
                } else {
                    GrpcArgument::Result(proto.result.unwrap_or(0))
                }
            }
            _ => GrpcArgument::GasCoin,
        }
    }
}

/// A Sui object from gRPC.
#[derive(Debug, Clone)]
pub struct GrpcObject {
    pub object_id: String,
    pub version: u64,
    pub digest: String,
    pub type_string: Option<String>,
    pub owner: GrpcOwner,
    /// Move struct BCS (starts with UID) - used for execution
    pub bcs: Option<Vec<u8>>,
    /// Full object BCS (includes type tag prefix) - for debugging
    pub bcs_full: Option<Vec<u8>>,
}

impl GrpcObject {
    fn from_proto(proto: proto::Object) -> Self {
        let owner = proto
            .owner
            .as_ref()
            .map(GrpcOwner::from_proto)
            .unwrap_or(GrpcOwner::Unknown);

        // Object type is a string in the proto
        let type_string = proto.object_type.clone();

        // "contents" field has Move struct BCS (starts with UID) - what we use for execution
        let contents = proto.contents.as_ref().and_then(|b| b.value.clone());

        // "bcs" field has full object BCS (includes type tag) - for debugging/comparison
        let bcs_full = proto.bcs.as_ref().and_then(|b| b.value.clone());

        // IMPORTANT: gRPC archive's "contents" field returns CURRENT state regardless
        // of version requested, but "bcs" field returns HISTORICAL state with a type tag prefix.
        // For accurate historical replay, we MUST extract from bcs_full, not contents.
        let object_id = proto.object_id.clone().unwrap_or_default();
        let bcs = bcs_full
            .as_ref()
            .and_then(|full_bcs| extract_move_struct_from_object_bcs(full_bcs, &object_id))
            .or(contents); // Fall back to contents only if extraction fails

        Self {
            object_id,
            version: proto.version.unwrap_or(0),
            digest: proto.digest.unwrap_or_default(),
            type_string,
            owner,
            bcs,
            bcs_full,
        }
    }
}

/// Extract Move struct BCS from full Object BCS by finding the UID (object_id).
///
/// The full Object BCS format is approximately:
/// - 1 byte: Object enum variant (0 = MoveObject, 1 = MovePackage)
/// - For MoveObject: StructTag (variable) + bool + u64 + Vec<u8> (contents)
///
/// The contents Vec<u8> has a ULEB128 length prefix followed by the actual struct bytes.
/// We find the contents by:
/// 1. Searching for the object_id (UID) which is at the start of Move struct data
/// 2. Reading the length prefix just before the UID to determine the exact size
fn extract_move_struct_from_object_bcs(bcs: &[u8], object_id: &str) -> Option<Vec<u8>> {
    // Parse object_id hex to bytes
    let id_hex = object_id.strip_prefix("0x").unwrap_or(object_id);
    let id_bytes = hex::decode(id_hex).ok()?;

    if id_bytes.len() != 32 {
        return None;
    }

    // Search for the object_id in the BCS data
    for i in 0..bcs.len().saturating_sub(32) {
        if &bcs[i..i + 32] == id_bytes.as_slice() {
            // Found the UID at position i
            // The Vec<u8> length prefix is just before the UID (1-3 bytes ULEB128)

            // Try reading ULEB128 starting from different positions before i
            // Start with 2 bytes (most common for structs 128-16K bytes)
            for prefix_start_offset in [2, 1, 3] {
                if i >= prefix_start_offset {
                    let len_start = i - prefix_start_offset;
                    if let Some((len, bytes_read)) = read_uleb128(&bcs[len_start..]) {
                        // Check if this ULEB128 ends exactly at position i
                        if len_start + bytes_read == i {
                            // Valid length prefix found
                            let contents_end = i + len;
                            if contents_end <= bcs.len() {
                                return Some(bcs[i..contents_end].to_vec());
                            }
                        }
                    }
                }
            }

            // Fallback: can't find valid length prefix, return from UID to end
            return Some(bcs[i..].to_vec());
        }
    }

    None
}

/// Read a ULEB128 encoded unsigned integer.
/// Returns (value, bytes_read) or None if invalid.
fn read_uleb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut result: usize = 0;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in data.iter().take(5) {
        // Max 5 bytes for usize
        bytes_read += 1;
        result |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Some((result, bytes_read));
        }
        shift += 7;
    }
    None
}

/// Object ownership.
#[derive(Debug, Clone)]
pub enum GrpcOwner {
    Address(String),
    Object(String),
    Shared { initial_version: u64 },
    Immutable,
    Unknown,
}

impl GrpcOwner {
    fn from_proto(proto: &proto::Owner) -> Self {
        use proto::owner::OwnerKind;

        match OwnerKind::try_from(proto.kind.unwrap_or(0)) {
            Ok(OwnerKind::Address) => GrpcOwner::Address(proto.address.clone().unwrap_or_default()),
            Ok(OwnerKind::Object) => {
                // Object owner uses the address field for the parent object ID
                GrpcOwner::Object(proto.address.clone().unwrap_or_default())
            }
            Ok(OwnerKind::Shared) => GrpcOwner::Shared {
                // version field holds initial_shared_version for shared objects
                initial_version: proto.version.unwrap_or(0),
            },
            Ok(OwnerKind::Immutable) => GrpcOwner::Immutable,
            _ => GrpcOwner::Unknown,
        }
    }
}
