use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sandbox_cli::network::{cache_dir, infer_network, resolve_graphql_endpoint};
use crate::sandbox_cli::SandboxState;
use sui_state_fetcher::{HistoricalStateProvider, ReplayState, ReplayStateConfig, VersionedCache};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;

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
    let api_key = std::env::var("SUI_GRPC_API_KEY")
        .ok()
        .or_else(|| dotenv.get("SUI_GRPC_API_KEY").cloned());
    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .or_else(|_| std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT"))
        .or_else(|_| std::env::var("SUI_GRPC_HISTORICAL_ENDPOINT"))
        .or_else(|_| {
            dotenv
                .get("SUI_GRPC_ENDPOINT")
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        })
        .or_else(|_| {
            dotenv
                .get("SUI_GRPC_ARCHIVE_ENDPOINT")
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        })
        .or_else(|_| {
            dotenv
                .get("SUI_GRPC_HISTORICAL_ENDPOINT")
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| state.rpc_url.clone());

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
    provider: &HistoricalStateProvider,
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
        .replay_state_builder()
        .with_config(replay_config)
        .build(digest)
        .await
        .context("Failed to fetch replay state")
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
