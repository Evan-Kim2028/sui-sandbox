use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sandbox_cli::network::{
    cache_dir, infer_network, resolve_graphql_endpoint, sandbox_home,
};
use crate::sandbox_cli::SandboxState;
use sui_state_fetcher::{
    FileStateProvider, HistoricalStateProvider, ReplayState, ReplayStateConfig,
    ReplayStateProvider, VersionedCache,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient};

use super::ReplaySource;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReplayHydrationConfig {
    pub prefetch_dynamic_fields: bool,
    pub prefetch_depth: usize,
    pub prefetch_limit: usize,
    pub auto_system_objects: bool,
}

pub(crate) async fn build_historical_state_provider(
    state: &SandboxState,
    source: ReplaySource,
    allow_fallback: bool,
    verbose: bool,
) -> Result<Arc<HistoricalStateProvider>> {
    let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
    let network = infer_network(&state.rpc_url, &graphql_endpoint);
    let cache = Arc::new(VersionedCache::with_storage(cache_dir(&network))?);

    let dotenv = load_dotenv_vars();
    let explicit_endpoint = std::env::var("SUI_GRPC_HISTORICAL_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT").ok())
        .or_else(|| std::env::var("SUI_GRPC_ENDPOINT").ok())
        .or_else(|| dotenv.get("SUI_GRPC_HISTORICAL_ENDPOINT").cloned())
        .or_else(|| dotenv.get("SUI_GRPC_ARCHIVE_ENDPOINT").cloned())
        .or_else(|| dotenv.get("SUI_GRPC_ENDPOINT").cloned())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let explicit_api_key = std::env::var("SUI_GRPC_API_KEY")
        .ok()
        .or_else(|| dotenv.get("SUI_GRPC_API_KEY").cloned())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let surflux_api_key = std::env::var("SURFLUX_API_KEY")
        .ok()
        .or_else(|| dotenv.get("SURFLUX_API_KEY").cloned())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (grpc_endpoint, api_key) = if let Some(endpoint) = explicit_endpoint {
        let key = explicit_api_key.or_else(|| {
            if endpoint.to_ascii_lowercase().contains("surflux.dev") {
                surflux_api_key.clone()
            } else {
                None
            }
        });
        (endpoint, key)
    } else {
        let (endpoint, key_from_env) = historical_endpoint_and_api_key_from_env();
        let key = key_from_env.or_else(|| {
            if endpoint.to_ascii_lowercase().contains("surflux.dev") {
                surflux_api_key
            } else {
                explicit_api_key
            }
        });
        (endpoint, key)
    };

    if verbose && grpc_endpoint != state.rpc_url {
        eprintln!("[grpc] using endpoint override {}", grpc_endpoint);
    }

    let grpc_client = GrpcClient::with_api_key(&grpc_endpoint, api_key).await?;
    let graphql_client = GraphQLClient::new(&graphql_endpoint);

    if matches!(source, ReplaySource::Walrus) && !allow_fallback {
        std::env::set_var("SUI_WALRUS_PACKAGE_ONLY", "1");
    }

    let mut provider =
        HistoricalStateProvider::with_clients(grpc_client, graphql_client).with_cache(cache);
    if matches!(source, ReplaySource::Walrus | ReplaySource::Hybrid) {
        provider = provider
            .with_walrus_from_env()
            .with_local_object_store_from_env();
    }

    Ok(Arc::new(provider))
}

pub(crate) async fn build_replay_state(
    provider: &impl ReplayStateProvider,
    digest: &str,
    config: ReplayHydrationConfig,
) -> Result<ReplayState> {
    let replay_config = ReplayStateConfig {
        prefetch_dynamic_fields: config.prefetch_dynamic_fields,
        df_depth: config.prefetch_depth,
        df_limit: config.prefetch_limit,
        auto_system_objects: config.auto_system_objects,
    };

    provider
        .fetch_replay_state_with_config(digest, &replay_config)
        .await
        .context("Failed to fetch replay state")
}

pub(crate) fn default_local_cache_dir() -> PathBuf {
    sandbox_home().join("cache").join("local")
}

pub(crate) fn build_local_state_provider(
    cache_dir: Option<&Path>,
) -> Result<Arc<FileStateProvider>> {
    let cache_dir = cache_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_local_cache_dir);
    let provider = FileStateProvider::new(&cache_dir).with_context(|| {
        format!(
            "Failed to initialize local replay cache {}",
            cache_dir.display()
        )
    })?;
    Ok(Arc::new(provider))
}

fn find_dotenv(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors().take(6) {
        let candidate = ancestor.join(".env");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn load_dotenv_vars() -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let Ok(start) = std::env::current_dir() else {
        return vars;
    };
    let Some(path) = find_dotenv(&start) else {
        return vars;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return vars;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k.trim(),
            None => continue,
        };
        let value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value)
            .to_string();
        vars.insert(key.to_string(), unquoted);
    }
    vars
}
