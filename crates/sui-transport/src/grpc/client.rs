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
    transaction_execution_service_client::TransactionExecutionServiceClient,
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
    api_key: Option<String>,
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
        Self::with_api_key(endpoint, None).await
    }

    /// Create a client with a custom endpoint and API key.
    /// The API key is included as an `x-api-key` header on all requests.
    pub async fn with_api_key(endpoint: &str, api_key: Option<String>) -> Result<Self> {
        use std::time::Duration;

        // Configure TLS for HTTPS endpoints with reasonable timeouts
        let channel = if endpoint.starts_with("https://") {
            Channel::from_shared(endpoint.to_string())?
                .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())?
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .connect()
                .await
                .map_err(|e| anyhow!("Failed to connect to gRPC endpoint {}: {}", endpoint, e))?
        } else {
            Channel::from_shared(endpoint.to_string())?
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .connect()
                .await
                .map_err(|e| anyhow!("Failed to connect to gRPC endpoint {}: {}", endpoint, e))?
        };

        Ok(Self {
            endpoint: endpoint.to_string(),
            channel,
            api_key,
        })
    }

    /// Wrap a request with the API key header if configured.
    fn wrap_request<T>(&self, req: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(req);
        if let Some(ref key) = self.api_key {
            if let Ok(value) = key.parse() {
                request.metadata_mut().insert("x-api-key", value);
            }
        }
        request
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
            .get_service_info(self.wrap_request(proto::GetServiceInfoRequest {}))
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
    // Transaction Simulation (Dev Inspect / Dry Run)
    // =========================================================================

    /// Simulate a transaction via the TransactionExecutionService.
    ///
    /// Use `checks = Disabled` for dev-inspect-like behavior and `checks = Enabled`
    /// for dry-run semantics. When checks are enabled, `do_gas_selection` controls
    /// whether the fullnode fills in gas payment/budget.
    pub async fn simulate_transaction(
        &self,
        transaction: proto::Transaction,
        checks: proto::simulate_transaction_request::TransactionChecks,
        do_gas_selection: bool,
    ) -> Result<proto::SimulateTransactionResponse> {
        let mut client = TransactionExecutionServiceClient::new(self.channel.clone());

        let request = proto::SimulateTransactionRequest {
            transaction: Some(transaction),
            read_mask: Some(prost_types::FieldMask {
                paths: vec!["*".to_string()],
            }),
            checks: Some(checks as i32),
            do_gas_selection: Some(do_gas_selection),
        };

        let response = client
            .simulate_transaction(self.wrap_request(request))
            .await
            .map_err(|e| anyhow!("gRPC error simulating transaction: {}", e))?;

        Ok(response.into_inner())
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
            .subscribe_checkpoints(self.wrap_request(request))
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
                    "package".to_string(),  // Package info including modules
                ],
            }),
        };

        let response = client
            .get_object(self.wrap_request(request))
            .await
            .map_err(|e| anyhow!("gRPC error fetching object: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.object.map(GrpcObject::from_proto))
    }

    /// Batch fetch multiple objects at specific versions with parallel execution.
    ///
    /// This is optimized for ground-truth prefetching where we know exact versions.
    /// Uses parallel requests with configurable concurrency to maximize throughput.
    ///
    /// # Arguments
    /// * `object_versions` - List of (object_id, version) pairs to fetch
    /// * `concurrency` - Maximum number of parallel requests (recommended: 10-20)
    ///
    /// # Returns
    /// Vec of (object_id, Result) pairs in the same order as input
    pub async fn batch_fetch_objects_at_versions(
        &self,
        object_versions: &[(String, u64)],
        concurrency: usize,
    ) -> Vec<(String, Result<Option<GrpcObject>>)> {
        use futures::stream::{self, StreamExt};

        let results: Vec<_> = stream::iter(object_versions.iter().cloned())
            .map(|(obj_id, version)| {
                let client = self.clone_for_batch();
                async move {
                    let result = client.get_object_at_version(&obj_id, Some(version)).await;
                    (obj_id, result)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        results
    }

    /// Batch fetch multiple objects at specific versions, returning only successful fetches.
    ///
    /// This is a convenience wrapper that filters out failures and returns a HashMap.
    pub async fn batch_fetch_objects_at_versions_ok(
        &self,
        object_versions: &[(String, u64)],
        concurrency: usize,
    ) -> std::collections::HashMap<String, GrpcObject> {
        let results = self
            .batch_fetch_objects_at_versions(object_versions, concurrency)
            .await;

        results
            .into_iter()
            .filter_map(|(id, result)| match result {
                Ok(Some(obj)) => Some((id, obj)),
                _ => None,
            })
            .collect()
    }

    /// Create a lightweight clone for parallel batch operations.
    /// Shares the underlying channel but creates a new client instance.
    fn clone_for_batch(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            channel: self.channel.clone(),
            api_key: self.api_key.clone(),
        }
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
            .batch_get_objects(self.wrap_request(request))
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
            .get_transaction(self.wrap_request(request))
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
            .batch_get_transactions(self.wrap_request(request))
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
            .get_checkpoint(self.wrap_request(request))
            .await
            .map_err(|e| anyhow!("gRPC error fetching checkpoint: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.checkpoint.map(GrpcCheckpoint::from_proto))
    }

    /// Fetch epoch information (protocol version, reference gas price, etc.).
    ///
    /// If `epoch` is None, returns the current epoch.
    pub async fn get_epoch(&self, epoch: Option<u64>) -> Result<Option<GrpcEpoch>> {
        let mut client = LedgerServiceClient::new(self.channel.clone());

        let request = proto::GetEpochRequest {
            epoch,
            read_mask: Some(prost_types::FieldMask {
                paths: vec![
                    "epoch".to_string(),
                    "system_state".to_string(),
                    "reference_gas_price".to_string(),
                    "protocol_config".to_string(),
                    "first_checkpoint".to_string(),
                    "last_checkpoint".to_string(),
                ],
            }),
        };

        let response = client
            .get_epoch(self.wrap_request(request))
            .await
            .map_err(|e| anyhow!("gRPC error fetching epoch: {}", e))?;

        let inner = response.into_inner();
        Ok(inner.epoch.map(GrpcEpoch::from_proto))
    }

    /// Fetch a package's modules at a specific version.
    ///
    /// This is useful for historical transaction replay where you need the exact
    /// bytecode that was deployed at the time of the transaction.
    pub async fn get_package_modules_at_version(
        &self,
        package_id: &str,
        version: Option<u64>,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let obj = self
            .get_object_at_version(package_id, version)
            .await?
            .ok_or_else(|| anyhow!("Package not found: {}", package_id))?;

        obj.package_modules
            .ok_or_else(|| anyhow!("Object {} is not a package", package_id))
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
            .get_checkpoint(self.wrap_request(request))
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
    /// The epoch this checkpoint belongs to.
    pub epoch: u64,
    pub transactions: Vec<GrpcTransaction>,
    /// Objects referenced as inputs or outputs in this checkpoint.
    pub objects: Vec<GrpcObject>,
}

/// Epoch metadata (protocol version, gas price, checkpoints).
#[derive(Debug, Clone)]
pub struct GrpcEpoch {
    pub epoch: u64,
    pub protocol_version: Option<u64>,
    pub reference_gas_price: Option<u64>,
    pub first_checkpoint: Option<u64>,
    pub last_checkpoint: Option<u64>,
}

impl GrpcCheckpoint {
    fn from_proto(proto: proto::Checkpoint) -> Self {
        let summary = proto.summary.as_ref();
        let timestamp_ms = summary
            .and_then(|s| s.timestamp.as_ref())
            .map(|t| (t.seconds as u64) * 1000 + (t.nanos as u64) / 1_000_000);
        let epoch = summary.and_then(|s| s.epoch).unwrap_or(0);
        let objects = proto
            .objects
            .map(|set| {
                set.objects
                    .into_iter()
                    .map(GrpcObject::from_proto)
                    .collect()
            })
            .unwrap_or_default();

        Self {
            sequence_number: proto.sequence_number.unwrap_or(0),
            digest: proto.digest.unwrap_or_default(),
            timestamp_ms,
            epoch,
            transactions: proto
                .transactions
                .into_iter()
                .map(|tx| GrpcTransaction::from_proto(tx).with_epoch(epoch))
                .collect(),
            objects,
        }
    }
}

impl GrpcEpoch {
    fn from_proto(proto: proto::Epoch) -> Self {
        let protocol_version = proto
            .system_state
            .as_ref()
            .and_then(|s| s.protocol_version)
            .or_else(|| {
                proto
                    .protocol_config
                    .as_ref()
                    .and_then(|c| c.protocol_version)
            });

        Self {
            epoch: proto.epoch.unwrap_or(0),
            protocol_version,
            reference_gas_price: proto.reference_gas_price,
            first_checkpoint: proto.first_checkpoint,
            last_checkpoint: proto.last_checkpoint,
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
    /// The epoch this transaction executed in.
    /// This is populated when fetched via checkpoint, None for direct transaction fetch.
    pub epoch: Option<u64>,
    pub inputs: Vec<GrpcInput>,
    pub commands: Vec<GrpcCommand>,
    pub status: Option<String>,
    /// Objects referenced as inputs or outputs by this transaction.
    pub objects: Vec<GrpcObject>,
    /// Detailed execution error information if the transaction failed.
    pub execution_error: Option<GrpcExecutionError>,
    /// Dynamic field children loaded but not modified during execution.
    /// Format: (object_id, version)
    pub unchanged_loaded_runtime_objects: Vec<(String, u64)>,
    /// Objects modified during execution with their INPUT versions (before tx).
    /// Format: (object_id, input_version) - only includes objects that existed before tx.
    pub changed_objects: Vec<(String, u64)>,
    /// Objects created during execution.
    /// Format: (object_id, output_version)
    pub created_objects: Vec<(String, u64)>,
    /// Consensus objects (shared objects) that were read but not modified.
    /// Format: (object_id, version) - this is the ACTUAL version used during execution,
    /// not the initial_shared_version from the transaction input.
    pub unchanged_consensus_objects: Vec<(String, u64)>,
}

/// Detailed execution error from a failed transaction.
#[derive(Debug, Clone)]
pub struct GrpcExecutionError {
    /// Human-readable error description.
    pub description: Option<String>,
    /// Command index where the error occurred.
    pub command: Option<u64>,
    /// Error kind (e.g., "MOVE_ABORT", "INSUFFICIENT_GAS").
    pub kind: Option<String>,
    /// Move abort details if this was a Move abort.
    pub move_abort: Option<GrpcMoveAbort>,
}

/// Move abort information from a failed transaction.
#[derive(Debug, Clone)]
pub struct GrpcMoveAbort {
    /// The abort code.
    pub abort_code: u64,
    /// Package where the abort occurred.
    pub package: Option<String>,
    /// Module where the abort occurred.
    pub module: Option<String>,
    /// Function name where the abort occurred.
    pub function_name: Option<String>,
    /// If this is a "clever error", the constant name used for the abort code.
    /// e.g., "E_INSUFFICIENT_BALANCE"
    pub constant_name: Option<String>,
    /// The rendered error message from the clever error.
    pub rendered_message: Option<String>,
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
        let objects = proto
            .objects
            .as_ref()
            .map(|set| {
                set.objects
                    .iter()
                    .cloned()
                    .map(GrpcObject::from_proto)
                    .collect()
            })
            .unwrap_or_default();

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

        // Parse execution error details including CleverError
        let execution_error = effects
            .and_then(|e| e.status.as_ref())
            .and_then(|s| s.error.as_ref())
            .map(|err| {
                use proto::execution_error::{ErrorDetails, ExecutionErrorKind};

                let kind = err.kind.and_then(|k| {
                    ExecutionErrorKind::try_from(k)
                        .ok()
                        .map(|k| format!("{:?}", k))
                });

                // Parse MoveAbort details including CleverError
                let move_abort = err.error_details.as_ref().and_then(|details| {
                    if let ErrorDetails::Abort(abort) = details {
                        let location = abort.location.as_ref();
                        let clever = abort.clever_error.as_ref();

                        Some(GrpcMoveAbort {
                            abort_code: abort.abort_code.unwrap_or(0),
                            package: location.and_then(|l| l.package.clone()),
                            module: location.and_then(|l| l.module.clone()),
                            function_name: location.and_then(|l| l.function_name.clone()),
                            constant_name: clever.and_then(|c| c.constant_name.clone()),
                            rendered_message: clever.and_then(|c| {
                                c.value.as_ref().and_then(|v| {
                                    if let proto::clever_error::Value::Rendered(s) = v {
                                        Some(s.clone())
                                    } else {
                                        None
                                    }
                                })
                            }),
                        })
                    } else {
                        None
                    }
                });

                GrpcExecutionError {
                    description: err.description.clone(),
                    command: err.command,
                    kind,
                    move_abort,
                }
            });

        // Extract unchanged_loaded_runtime_objects from effects
        let unchanged_loaded_runtime_objects = effects
            .map(|e| {
                e.unchanged_loaded_runtime_objects
                    .iter()
                    .filter_map(|obj_ref| {
                        let object_id = obj_ref.object_id.clone()?;
                        let version = obj_ref.version?;
                        Some((object_id, version))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Extract changed_objects with their INPUT versions (before tx)
        // These are objects that were MODIFIED during execution
        let changed_objects = effects
            .map(|e| {
                use super::generated::sui_rpc_v2::changed_object::InputObjectState;
                e.changed_objects
                    .iter()
                    .filter_map(|obj| {
                        let object_id = obj.object_id.clone()?;
                        // Only include objects that existed before the transaction
                        let input_state = obj.input_state?;
                        if input_state == InputObjectState::Exists as i32 {
                            let input_version = obj.input_version?;
                            Some((object_id, input_version))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Extract created_objects (objects that didn't exist before tx)
        let created_objects = effects
            .map(|e| {
                use super::generated::sui_rpc_v2::changed_object::InputObjectState;
                e.changed_objects
                    .iter()
                    .filter_map(|obj| {
                        let object_id = obj.object_id.clone()?;
                        let input_state = obj.input_state?;
                        // Objects that didn't exist before the transaction
                        if input_state == InputObjectState::DoesNotExist as i32 {
                            let output_version = obj.output_version?;
                            Some((object_id, output_version))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Extract unchanged_consensus_objects (shared objects read but not modified)
        // This is CRITICAL for replay - the version here is the ACTUAL version used during
        // execution, not the initial_shared_version from the transaction input!
        let unchanged_consensus_objects = effects
            .map(|e| {
                e.unchanged_consensus_objects
                    .iter()
                    .filter_map(|obj| {
                        let object_id = obj.object_id.clone()?;
                        let version = obj.version?;
                        Some((object_id, version))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            digest: proto.digest.unwrap_or_default(),
            sender: tx.and_then(|t| t.sender.clone()).unwrap_or_default(),
            gas_budget: gas_payment.and_then(|g| g.budget),
            gas_price: gas_payment.and_then(|g| g.price),
            checkpoint: proto.checkpoint,
            timestamp_ms,
            epoch: None, // Will be set by checkpoint when fetched via checkpoint
            inputs,
            commands,
            status,
            objects,
            execution_error,
            unchanged_loaded_runtime_objects,
            changed_objects,
            created_objects,
            unchanged_consensus_objects,
        }
    }

    /// Create a copy with the epoch set.
    pub fn with_epoch(mut self, epoch: u64) -> Self {
        self.epoch = Some(epoch);
        self
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

/// Package linkage entry mapping original_id -> upgraded_id
#[derive(Debug, Clone)]
pub struct GrpcLinkage {
    pub original_id: String,
    pub upgraded_id: String,
    pub upgraded_version: u64,
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
    /// Package modules (if this object is a package)
    /// Each tuple is (module_name, module_bytecode)
    pub package_modules: Option<Vec<(String, Vec<u8>)>>,
    /// Package linkage table (if this object is a package)
    /// Maps original package IDs to their upgraded storage IDs
    pub package_linkage: Option<Vec<GrpcLinkage>>,
    /// Package original_id (for upgraded packages)
    /// This is the storage_id of the first version of this package.
    /// For the first version, this equals the object_id.
    pub package_original_id: Option<String>,
    /// The digest of the transaction that created or last mutated this object.
    /// For packages, this is the publish transaction.
    pub previous_transaction: Option<String>,
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

        // Extract package modules if this is a package
        let package_modules = proto.package.as_ref().map(|pkg| {
            pkg.modules
                .iter()
                .filter_map(|m| {
                    let name = m.name.clone()?;
                    let contents = m.contents.clone()?;
                    Some((name, contents))
                })
                .collect()
        });

        // Extract package linkage table if this is a package
        let package_linkage = proto.package.as_ref().map(|pkg| {
            pkg.linkage
                .iter()
                .filter_map(|l| {
                    let original_id = l.original_id.clone()?;
                    let upgraded_id = l.upgraded_id.clone()?;
                    let upgraded_version = l.upgraded_version.unwrap_or(0);
                    Some(GrpcLinkage {
                        original_id,
                        upgraded_id,
                        upgraded_version,
                    })
                })
                .collect()
        });

        // Extract package original_id (the first published version ID)
        // This is critical for package upgrades - the original_id is stable
        // across all versions and is used for type address resolution.
        let package_original_id = proto
            .package
            .as_ref()
            .and_then(|pkg| pkg.original_id.clone());

        Self {
            object_id,
            version: proto.version.unwrap_or(0),
            digest: proto.digest.unwrap_or_default(),
            type_string,
            owner,
            bcs,
            bcs_full,
            package_modules,
            package_linkage,
            package_original_id,
            previous_transaction: proto.previous_transaction.clone(),
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

    // Collect ALL positions where the object_id appears in the BCS data
    let mut matches: Vec<usize> = Vec::new();
    for i in 0..bcs.len().saturating_sub(32) {
        if &bcs[i..i + 32] == id_bytes.as_slice() {
            matches.push(i);
        }
    }

    if matches.is_empty() {
        return None;
    }

    // Try each match position, looking for the best ULEB128 length prefix
    // The correct position will have a ULEB128 that:
    // 1. Ends exactly at the UID position
    // 2. Specifies a length that fits within the remaining BCS data
    // We prefer LARGER lengths because smaller lengths might be coincidental byte patterns
    let mut best_result: Option<(usize, usize, usize)> = None; // (pos, len, offset)

    for &i in &matches {
        // Try reading ULEB128 starting from different positions before i
        // Try offsets 1-5 bytes (ULEB128 can encode up to 2^35 with 5 bytes)
        for prefix_start_offset in 1..=5 {
            if i >= prefix_start_offset {
                let len_start = i - prefix_start_offset;
                if let Some((len, bytes_read)) = read_uleb128(&bcs[len_start..]) {
                    // Check if this ULEB128 ends exactly at position i
                    if len_start + bytes_read == i {
                        // Valid length prefix found
                        let contents_end = i + len;
                        if contents_end <= bcs.len() {
                            // Additional validation: len should be substantial (at least a UID)
                            if len >= 32 {
                                // Prefer the largest valid length we find
                                if best_result.is_none() || len > best_result.unwrap().1 {
                                    best_result = Some((i, len, prefix_start_offset));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some((i, len, _offset)) = best_result {
        let contents_end = i + len;
        return Some(bcs[i..contents_end].to_vec());
    }

    // Fallback: use the first match position and return from UID to end
    // This handles cases where we can't find a valid ULEB128 prefix
    let i = matches[0];
    Some(bcs[i..].to_vec())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::test_utils::GrpcTransactionBuilder;

    // =========================================================================
    // GrpcOwner parsing tests (consolidated)
    // =========================================================================

    #[test]
    fn test_grpc_owner_variants() {
        use proto::owner::OwnerKind;

        // Address variant
        let proto = proto::Owner {
            kind: Some(OwnerKind::Address as i32),
            address: Some("0x1234".to_string()),
            version: None,
        };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Address(a) if a == "0x1234"));

        // Object variant
        let proto = proto::Owner {
            kind: Some(OwnerKind::Object as i32),
            address: Some("0xparent".to_string()),
            version: None,
        };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Object(p) if p == "0xparent"));

        // Shared variant
        let proto = proto::Owner {
            kind: Some(OwnerKind::Shared as i32),
            address: None,
            version: Some(42),
        };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Shared { initial_version: 42 }));

        // Immutable variant
        let proto = proto::Owner {
            kind: Some(OwnerKind::Immutable as i32),
            address: None,
            version: None,
        };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Immutable));
    }

    #[test]
    fn test_grpc_owner_edge_cases() {
        use proto::owner::OwnerKind;

        // Invalid kind -> Unknown
        let proto = proto::Owner { kind: Some(999), address: None, version: None };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Unknown));

        // None kind -> Unknown
        let proto = proto::Owner { kind: None, address: None, version: None };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Unknown));

        // Address with missing address field -> empty string
        let proto = proto::Owner {
            kind: Some(OwnerKind::Address as i32),
            address: None,
            version: None,
        };
        assert!(matches!(GrpcOwner::from_proto(&proto), GrpcOwner::Address(a) if a.is_empty()));
    }

    // =========================================================================
    // GrpcTransaction is_ptb tests (using builder)
    // =========================================================================

    #[test]
    fn test_grpc_transaction_is_ptb() {
        // With commands and sender -> PTB
        let tx = GrpcTransactionBuilder::new()
            .with_move_call("0x2", "coin", "value")
            .build();
        assert!(tx.is_ptb(), "Transaction with MoveCall is a PTB");

        // With gas budget only -> PTB
        let tx = GrpcTransactionBuilder::new().build();
        assert!(tx.is_ptb(), "Transaction with gas budget is a PTB");
    }

    #[test]
    fn test_grpc_transaction_is_not_ptb() {
        // Empty sender -> not PTB (system transaction)
        let tx = GrpcTransactionBuilder::new().sender("").build();
        assert!(!tx.is_ptb(), "Empty sender is not a PTB");

        // No commands and no gas budget -> not PTB
        let tx = GrpcTransactionBuilder::minimal()
            .sender("0x1")
            .gas_budget(None)
            .build();
        assert!(!tx.is_ptb(), "No commands and no gas is not a PTB");
    }

    // =========================================================================
    // GrpcArgument parsing tests (consolidated)
    // =========================================================================

    #[test]
    fn test_grpc_argument_variants() {
        use proto::argument::ArgumentKind;

        // Gas
        let proto = proto::Argument {
            kind: Some(ArgumentKind::Gas as i32),
            input: None, result: None, subresult: None,
        };
        assert!(matches!(GrpcArgument::from_proto(&proto), GrpcArgument::GasCoin));

        // Input
        let proto = proto::Argument {
            kind: Some(ArgumentKind::Input as i32),
            input: Some(5), result: None, subresult: None,
        };
        assert!(matches!(GrpcArgument::from_proto(&proto), GrpcArgument::Input(5)));

        // Result
        let proto = proto::Argument {
            kind: Some(ArgumentKind::Result as i32),
            input: None, result: Some(3), subresult: None,
        };
        assert!(matches!(GrpcArgument::from_proto(&proto), GrpcArgument::Result(3)));

        // NestedResult (Result with subresult)
        let proto = proto::Argument {
            kind: Some(ArgumentKind::Result as i32),
            input: None, result: Some(2), subresult: Some(1),
        };
        assert!(matches!(GrpcArgument::from_proto(&proto), GrpcArgument::NestedResult(2, 1)));

        // None kind defaults to Gas
        let proto = proto::Argument { kind: None, input: None, result: None, subresult: None };
        assert!(matches!(GrpcArgument::from_proto(&proto), GrpcArgument::GasCoin));
    }

    // =========================================================================
    // GrpcInput parsing tests (consolidated)
    // =========================================================================

    #[test]
    fn test_grpc_input_variants() {
        use proto::input::InputKind;

        // Pure
        let proto = proto::Input {
            kind: Some(InputKind::Pure as i32),
            pure: Some(vec![1, 2, 3]),
            ..Default::default()
        };
        assert!(matches!(GrpcInput::from_proto(&proto), GrpcInput::Pure { bytes } if bytes == vec![1, 2, 3]));

        // ImmutableOrOwned -> Object
        let proto = proto::Input {
            kind: Some(InputKind::ImmutableOrOwned as i32),
            object_id: Some("0xobj".to_string()),
            version: Some(10),
            digest: Some("abc".to_string()),
            ..Default::default()
        };
        match GrpcInput::from_proto(&proto) {
            GrpcInput::Object { object_id, version, digest } => {
                assert_eq!(object_id, "0xobj");
                assert_eq!(version, 10);
                assert_eq!(digest, "abc");
            }
            _ => panic!("Expected Object input"),
        }

        // Shared
        let proto = proto::Input {
            kind: Some(InputKind::Shared as i32),
            object_id: Some("0xshared".to_string()),
            version: Some(5),
            mutable: Some(true),
            ..Default::default()
        };
        match GrpcInput::from_proto(&proto) {
            GrpcInput::SharedObject { object_id, initial_version, mutable } => {
                assert_eq!(object_id, "0xshared");
                assert_eq!(initial_version, 5);
                assert!(mutable);
            }
            _ => panic!("Expected SharedObject input"),
        }

        // Receiving
        let proto = proto::Input {
            kind: Some(InputKind::Receiving as i32),
            object_id: Some("0xrecv".to_string()),
            version: Some(7),
            digest: Some("def".to_string()),
            ..Default::default()
        };
        match GrpcInput::from_proto(&proto) {
            GrpcInput::Receiving { object_id, version, digest } => {
                assert_eq!(object_id, "0xrecv");
                assert_eq!(version, 7);
                assert_eq!(digest, "def");
            }
            _ => panic!("Expected Receiving input"),
        }

        // None kind defaults to Pure
        let proto = proto::Input::default();
        assert!(matches!(GrpcInput::from_proto(&proto), GrpcInput::Pure { bytes } if bytes.is_empty()));
    }

    // =========================================================================
    // GrpcCommand variant tests (consolidated)
    // =========================================================================

    #[test]
    fn test_grpc_command_variants() {
        // MoveCall
        let cmd = GrpcCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_arguments: vec!["0x2::sui::SUI".to_string()],
            arguments: vec![GrpcArgument::Input(0)],
        };
        match cmd {
            GrpcCommand::MoveCall { package, module, function, type_arguments, arguments } => {
                assert_eq!(package, "0x2");
                assert_eq!(module, "coin");
                assert_eq!(function, "value");
                assert_eq!(type_arguments.len(), 1);
                assert_eq!(arguments.len(), 1);
            }
            _ => panic!("Expected MoveCall"),
        }

        // SplitCoins
        let cmd = GrpcCommand::SplitCoins {
            coin: GrpcArgument::Input(0),
            amounts: vec![GrpcArgument::Input(1), GrpcArgument::Input(2)],
        };
        assert!(matches!(cmd, GrpcCommand::SplitCoins { coin: GrpcArgument::Input(0), amounts } if amounts.len() == 2));

        // TransferObjects
        let cmd = GrpcCommand::TransferObjects {
            objects: vec![GrpcArgument::Result(0)],
            address: GrpcArgument::Input(1),
        };
        assert!(matches!(cmd, GrpcCommand::TransferObjects { objects, address: GrpcArgument::Input(1) } if objects.len() == 1));
    }
}
