use crate::logging::{redact_sensitive, LogConfig, LogRecord, McpLogger};
use crate::paths::default_paths;
use crate::project::ProjectManager;
use crate::world::WorldManager;
use anyhow::Result;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_state_fetcher::cache::VersionedCache;
use sui_state_fetcher::HistoricalStateProvider;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;
use sui_transport::network::{
    default_graphql_endpoint, infer_network_from_endpoints, resolve_graphql_endpoint,
};

// Re-export shared types for use by tools.rs
pub use sui_sandbox_core::shared::{ToolMeta, ToolResponse};

#[derive(Debug, Clone)]
struct ObjectHandle {
    pub id: String,
}

#[derive(Debug, Default)]
struct ObjectRefStore {
    counter: u64,
    refs: HashMap<String, ObjectHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub network: String,
    pub grpc_endpoint: Option<String>,
    pub graphql_endpoint: Option<String>,
}

impl ProviderConfig {
    pub fn with_defaults(mut self) -> Self {
        let inferred = infer_network_from_endpoints(
            self.grpc_endpoint.as_deref(),
            self.graphql_endpoint.as_deref(),
        );
        if self.network.is_empty() {
            self.network = inferred.unwrap_or("mainnet").to_string();
        }
        if self
            .graphql_endpoint
            .as_ref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
        {
            if let Some(rpc) = self.grpc_endpoint.as_deref() {
                self.graphql_endpoint = Some(resolve_graphql_endpoint(rpc));
            } else {
                let env_graphql = std::env::var("SUI_GRAPHQL_ENDPOINT")
                    .ok()
                    .filter(|s| !s.trim().is_empty());
                self.graphql_endpoint =
                    env_graphql.or_else(|| Some(default_graphql_endpoint(&self.network)));
            }
        }
        self
    }
}

#[derive(Clone)]
struct ProviderState {
    config: ProviderConfig,
    provider: Option<Arc<HistoricalStateProvider>>,
}

#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    pub fork_anchor: Option<Value>,
}

pub struct ToolDispatcher {
    pub env: Arc<Mutex<SimulationEnvironment>>,
    pub projects: ProjectManager,
    pub worlds: WorldManager,
    pub logger: McpLogger,
    cache_root: PathBuf,
    caches: Mutex<HashMap<String, Arc<VersionedCache>>>,
    provider: tokio::sync::Mutex<ProviderState>,
    object_refs: Mutex<ObjectRefStore>,
    config: Mutex<DispatcherConfig>,
}

impl ToolDispatcher {
    pub fn new() -> Result<Self> {
        let mut env = SimulationEnvironment::new()?;
        let projects = ProjectManager::new(None)?;
        let worlds = WorldManager::new(None)?;
        let logger = McpLogger::new(LogConfig::default());
        let cache_root = default_paths().cache_dir();
        let provider = ProviderState {
            config: ProviderConfig {
                network: "mainnet".to_string(),
                grpc_endpoint: None,
                graphql_endpoint: None,
            },
            provider: None,
        };

        // Auto-resume: if there's an active world with saved state, load it
        if let Some(world_id) = worlds.active_id() {
            if let Ok(Some(state_data)) = worlds.load_world_state(&world_id) {
                if let Err(e) = env.load_state_from_bytes(&state_data) {
                    eprintln!("Warning: failed to restore world state: {}", e);
                }
            }
        }

        let env = Arc::new(Mutex::new(env));

        Ok(Self {
            env,
            projects,
            worlds,
            logger,
            cache_root,
            caches: Mutex::new(HashMap::new()),
            provider: tokio::sync::Mutex::new(provider),
            object_refs: Mutex::new(ObjectRefStore::default()),
            config: Mutex::new(DispatcherConfig { fork_anchor: None }),
        })
    }

    pub fn logger(&self) -> &McpLogger {
        &self.logger
    }

    pub(crate) fn clear_object_refs(&self) {
        let mut refs = self.object_refs.lock();
        refs.counter = 0;
        refs.refs.clear();
    }

    pub async fn set_provider_config(&self, config: ProviderConfig) {
        let mut guard = self.provider.lock().await;
        guard.config = config.with_defaults();
        guard.provider = None;
    }

    pub async fn provider_config(&self) -> ProviderConfig {
        self.provider.lock().await.config.clone()
    }

    pub async fn provider(&self) -> Result<Arc<HistoricalStateProvider>> {
        let mut guard = self.provider.lock().await;
        if let Some(provider) = guard.provider.clone() {
            return Ok(provider);
        }

        let cache = self.cache_for_network(&guard.config.network)?;
        let provider = if let Some(grpc) = &guard.config.grpc_endpoint {
            let graphql = guard
                .config
                .graphql_endpoint
                .clone()
                .unwrap_or_else(|| default_graphql_endpoint(&guard.config.network));
            let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
            let grpc_client = GrpcClient::with_api_key(grpc, api_key).await?;
            let graphql_client = GraphQLClient::new(&graphql);
            HistoricalStateProvider::with_clients(grpc_client, graphql_client)
        } else {
            match guard.config.network.as_str() {
                "testnet" => HistoricalStateProvider::testnet().await?,
                _ => HistoricalStateProvider::mainnet().await?,
            }
        };
        let provider = provider
            .with_walrus_from_env()
            .with_local_object_store_from_env()
            .with_cache(cache.clone());
        let provider = Arc::new(provider);
        guard.provider = Some(provider.clone());
        Ok(provider)
    }

    pub fn cache_for_network(&self, network: &str) -> Result<Arc<VersionedCache>> {
        let mut guard = self.caches.lock();
        if let Some(cache) = guard.get(network) {
            return Ok(cache.clone());
        }
        let path = self.cache_root.join(network);
        let cache = Arc::new(VersionedCache::with_storage(path)?);
        guard.insert(network.to_string(), cache.clone());
        Ok(cache)
    }

    pub fn register_object_ref(&self, object_id: &str) -> String {
        let mut store = self.object_refs.lock();
        store.counter += 1;
        let handle = format!("obj_{}", store.counter);
        store.refs.insert(
            handle.clone(),
            ObjectHandle {
                id: object_id.to_string(),
            },
        );
        handle
    }

    pub(crate) fn resolve_object_ref(&self, object_ref: &str) -> Option<String> {
        self.object_refs
            .lock()
            .refs
            .get(object_ref)
            .map(|h| h.id.clone())
    }

    pub fn reset_object_refs(&self) {
        let mut refs = self.object_refs.lock();
        refs.counter = 0;
        refs.refs.clear();
    }

    pub fn set_fork_anchor(&self, anchor: Option<Value>) {
        self.config.lock().fork_anchor = anchor;
    }

    pub fn fork_anchor(&self) -> Option<Value> {
        self.config.lock().fork_anchor.clone()
    }

    /// Graceful shutdown - saves active world state and session
    pub fn shutdown(&self) -> Result<()> {
        // Save active world state if any
        if let Some(world) = self.worlds.active() {
            let env = self.env.lock();
            if let Ok(state_bytes) = env.save_state_to_bytes() {
                let _ = self.worlds.save_world_state(&world.id, &state_bytes);
            }
        }
        // Save session state
        self.worlds.shutdown()
    }

    /// Get the current session info
    pub fn session_info(&self) -> crate::world::Session {
        self.worlds.get_session()
    }

    pub async fn dispatch(&self, tool: &str, input: Value) -> ToolResponse {
        let (meta, clean_input) = extract_meta(&input);
        let request_id = meta
            .request_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let start = Instant::now();

        let mut result = self.dispatch_inner(tool, clean_input.clone()).await;
        let state_file = default_paths().base_dir().join("mcp-state.json");
        result.state_file = Some(state_file.to_string_lossy().to_string());

        let duration_ms = start.elapsed().as_millis();
        let record = LogRecord {
            ts: Utc::now().to_rfc3339(),
            request_id,
            tool: tool.to_string(),
            input: redact_sensitive(&clean_input),
            output: redact_sensitive(&serde_json::to_value(&result).unwrap_or(Value::Null)),
            duration_ms,
            success: result.success,
            error: result.error.clone(),
            cache_hit: result.cache_hit,
            llm_reason: meta.reason.clone(),
            tags: meta.tags.clone(),
        };
        let _ = self.logger.log_tool_call(&record);

        result
    }

    async fn dispatch_inner(&self, tool: &str, input: Value) -> ToolResponse {
        match tool {
            // Execution tools
            "call_function" => self.call_function(input).await,
            "execute_ptb" => self.execute_ptb(input).await,
            "replay_transaction" => self.replay_transaction(input).await,
            "publish" => self.publish(input).await,
            "run" => self.run(input).await,
            "ptb" => self.ptb(input).await,
            "fetch" => self.load_from_mainnet(input).await,
            "replay" => self.replay_transaction(input).await,
            "view" => self.view(input).await,
            "bridge" => self.bridge(input).await,
            "status" => self.status(input).await,
            "clean" => self.clean(input).await,

            // Legacy project tools (still supported)
            "create_move_project" => self.create_move_project(input).await,
            "read_move_file" => self.read_move_file(input).await,
            "edit_move_file" => self.edit_move_file(input).await,
            "build_project" => self.build_project(input).await,
            "test_project" => self.test_project(input).await,
            "deploy_project" => self.deploy_project(input).await,
            "list_projects" => self.list_projects(input).await,
            "list_packages" => self.list_packages(input).await,
            "set_active_package" => self.set_active_package(input).await,
            "upgrade_project" => self.upgrade_project(input).await,

            // World management tools
            "world_create" => self.world_create(input).await,
            "world_open" => self.world_open(input).await,
            "world_close" => self.world_close(input).await,
            "world_list" => self.world_list(input).await,
            "world_status" => self.world_status(input).await,
            "world_delete" => self.world_delete(input).await,
            "world_snapshot" => self.world_snapshot(input).await,
            "world_restore" => self.world_restore(input).await,
            "world_build" => self.world_build(input).await,
            "world_deploy" => self.world_deploy(input).await,
            "world_commit" => self.world_commit(input).await,
            "world_log" => self.world_log(input).await,
            "world_templates" => self.world_templates(input).await,
            "world_export" => self.world_export(input).await,
            "world_read_file" => self.world_read_file(input).await,
            "world_write_file" => self.world_write_file(input).await,

            // State/object tools
            "read_object" => self.read_object(input).await,
            "create_asset" => self.create_asset(input).await,
            "load_from_mainnet" => self.load_from_mainnet(input).await,
            "load_package_bytes" => self.load_package_bytes(input).await,
            "get_interface" => self.get_interface(input).await,
            "search" => self.search(input).await,
            "get_state" => self.get_state(input).await,
            "configure" => self.configure(input).await,
            "walrus_fetch_checkpoints" => self.walrus_fetch_checkpoints(input).await,

            // Transaction history tools
            "history_list" => self.history_list(input).await,
            "history_get" => self.history_get(input).await,
            "history_search" => self.history_search(input).await,
            "history_summary" => self.history_summary(input).await,
            "history_configure" => self.history_configure(input).await,

            _ => ToolResponse::error(format!("Unknown tool: {}", tool)),
        }
    }
}

fn extract_meta(input: &Value) -> (ToolMeta, Value) {
    let mut meta = ToolMeta::default();
    if let Value::Object(map) = input {
        if let Some(Value::Object(meta_map)) = map.get("_meta") {
            if let Some(Value::String(reason)) = meta_map.get("reason") {
                meta.reason = Some(reason.clone());
            }
            if let Some(Value::String(req)) = meta_map.get("request_id") {
                meta.request_id = Some(req.clone());
            }
            if let Some(Value::Array(tags)) = meta_map.get("tags") {
                let parsed: Vec<String> = tags
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if !parsed.is_empty() {
                    meta.tags = Some(parsed);
                }
            }
        }

        let mut cleaned = map.clone();
        cleaned.remove("_meta");
        return (meta, Value::Object(cleaned));
    }
    (meta, input.clone())
}
