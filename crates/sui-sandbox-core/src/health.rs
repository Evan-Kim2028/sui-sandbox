use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient};
use sui_transport::network::{infer_network_from_endpoints, resolve_graphql_endpoint};
use sui_transport::walrus::WalrusClient;

const WALRUS_MAINNET_CACHE_URL: &str = "https://walrus-sui-archival.mainnet.walrus.space";
const WALRUS_MAINNET_AGGREGATOR_URL: &str = "https://aggregator.walrus-mainnet.walrus.space";
const WALRUS_TESTNET_CACHE_URL: &str = "https://walrus-sui-archival.testnet.walrus.space";
const WALRUS_TESTNET_AGGREGATOR_URL: &str = "https://aggregator.walrus-testnet.walrus.space";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub id: String,
    pub name: String,
    pub status: DoctorStatus,
    pub passed: bool,
    pub detail: String,
    pub remediation: Option<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub passed: usize,
    pub failed: usize,
    pub grpc_endpoint: String,
    pub graphql_endpoint: String,
    pub walrus_cache_url: String,
    pub walrus_aggregator_url: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone)]
pub struct DoctorConfig {
    pub timeout_secs: u64,
    pub rpc_url: String,
    pub state_file: PathBuf,
    pub include_toolchain_checks: bool,
}

fn pass_check(id: &str, name: &str, detail: String, start: Instant) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        name: name.to_string(),
        status: DoctorStatus::Pass,
        passed: true,
        detail,
        remediation: None,
        duration_ms: start.elapsed().as_millis(),
    }
}

fn fail_check(
    id: &str,
    name: &str,
    detail: String,
    remediation: &str,
    start: Instant,
) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        name: name.to_string(),
        status: DoctorStatus::Fail,
        passed: false,
        detail,
        remediation: Some(remediation.to_string()),
        duration_ms: start.elapsed().as_millis(),
    }
}

fn run_version_command(binary: &str) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to execute `{binary} --version`"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "`{binary} --version` exited with status {}: {}",
            output.status,
            stderr
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn check_state_file_permissions(state_file: &Path) -> Result<String> {
    let parent = state_file
        .parent()
        .ok_or_else(|| anyhow!("state file has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create state-file directory {}", parent.display()))?;

    let existed = state_file.exists();
    let _file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_file)
        .with_context(|| format!("open state file {}", state_file.display()))?;

    if !existed {
        let _ = std::fs::remove_file(state_file);
    }

    Ok(format!("state file is writable: {}", state_file.display()))
}

fn walrus_urls_for_network(network: &str) -> (String, String) {
    let default_cache = match network {
        "testnet" => WALRUS_TESTNET_CACHE_URL,
        _ => WALRUS_MAINNET_CACHE_URL,
    };
    let default_agg = match network {
        "testnet" => WALRUS_TESTNET_AGGREGATOR_URL,
        _ => WALRUS_MAINNET_AGGREGATOR_URL,
    };

    let cache = std::env::var("SUI_WALRUS_CACHE_URL").unwrap_or_else(|_| default_cache.to_string());
    let agg =
        std::env::var("SUI_WALRUS_AGGREGATOR_URL").unwrap_or_else(|_| default_agg.to_string());
    (cache, agg)
}

pub async fn run_doctor(config: &DoctorConfig) -> Result<DoctorReport> {
    let graphql_endpoint = resolve_graphql_endpoint(&config.rpc_url);
    let (grpc_endpoint, grpc_api_key) = historical_endpoint_and_api_key_from_env();
    let network = infer_network_from_endpoints(Some(&config.rpc_url), Some(&graphql_endpoint))
        .unwrap_or("mainnet");
    let (walrus_cache_url, walrus_aggregator_url) = walrus_urls_for_network(network);

    let mut checks = Vec::new();

    if config.include_toolchain_checks {
        let rust_start = Instant::now();
        let rust_result = (|| -> Result<String> {
            let rustc = run_version_command("rustc")?;
            let cargo = run_version_command("cargo")?;
            Ok(format!("{rustc}; {cargo}"))
        })();
        checks.push(match rust_result {
            Ok(detail) => pass_check("rust_toolchain", "Rust Toolchain", detail, rust_start),
            Err(err) => fail_check(
                "rust_toolchain",
                "Rust Toolchain",
                err.to_string(),
                "Install Rust with rustup (https://rustup.rs) and ensure `rustc`/`cargo` are on PATH.",
                rust_start,
            ),
        });

        let sui_start = Instant::now();
        checks.push(match run_version_command("sui") {
            Ok(detail) => pass_check("sui_cli", "Sui CLI", detail, sui_start),
            Err(err) => fail_check(
                "sui_cli",
                "Sui CLI",
                err.to_string(),
                "Install Sui CLI and ensure `sui` is on PATH.",
                sui_start,
            ),
        });
    }

    let state_start = Instant::now();
    checks.push(match check_state_file_permissions(&config.state_file) {
        Ok(detail) => pass_check(
            "state_file_permissions",
            "State File Permissions",
            detail,
            state_start,
        ),
        Err(err) => fail_check(
            "state_file_permissions",
            "State File Permissions",
            err.to_string(),
            "Use `--state-file` with a writable path and verify directory permissions.",
            state_start,
        ),
    });

    let env_start = Instant::now();
    let env_detail = format!(
        "historical endpoint={} api_key_configured={}",
        grpc_endpoint,
        grpc_api_key
            .as_ref()
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    );
    if grpc_endpoint
        .to_ascii_lowercase()
        .contains("archive.mainnet.sui.io")
    {
        checks.push(pass_check(
            "grpc_env",
            "gRPC Env Configuration",
            format!(
                "{} (note: if replay misses unchanged runtime objects, consider setting `SUI_GRPC_HISTORICAL_ENDPOINT` or provider-specific archival endpoints)",
                env_detail
            ),
            env_start,
        ));
    } else if grpc_endpoint.to_ascii_lowercase().contains("surflux.dev") && grpc_api_key.is_none() {
        checks.push(fail_check(
            "grpc_env",
            "gRPC Env Configuration",
            env_detail,
            "Set `SURFLUX_API_KEY` (or `SUI_GRPC_API_KEY`) when using the Surflux endpoint.",
            env_start,
        ));
    } else {
        checks.push(pass_check(
            "grpc_env",
            "gRPC Env Configuration",
            env_detail,
            env_start,
        ));
    }

    let grpc_start = Instant::now();
    let grpc_timeout = Duration::from_secs(config.timeout_secs);
    let grpc_check = tokio::time::timeout(grpc_timeout, async {
        let client = GrpcClient::with_api_key(&grpc_endpoint, grpc_api_key.clone()).await?;
        let info = client.get_service_info().await?;
        Ok::<String, anyhow::Error>(format!(
            "connected (chain={}, epoch={}, checkpoint={})",
            info.chain, info.epoch, info.checkpoint_height
        ))
    })
    .await;
    checks.push(match grpc_check {
        Ok(Ok(detail)) => pass_check("grpc_reachability", "gRPC Reachability", detail, grpc_start),
        Ok(Err(err)) => fail_check(
            "grpc_reachability",
            "gRPC Reachability",
            err.to_string(),
            "Set `SUI_GRPC_HISTORICAL_ENDPOINT`/`SUI_GRPC_ARCHIVE_ENDPOINT` to a reachable endpoint and verify API key settings.",
            grpc_start,
        ),
        Err(_) => fail_check(
            "grpc_reachability",
            "gRPC Reachability",
            format!("timed out after {}s", config.timeout_secs),
            "Check network/firewall access and endpoint correctness.",
            grpc_start,
        ),
    });

    let graphql_start = Instant::now();
    let gql_client = GraphQLClient::with_timeouts(
        &graphql_endpoint,
        Duration::from_secs(config.timeout_secs),
        Duration::from_secs(config.timeout_secs.min(10)),
    );
    checks.push(match gql_client.fetch_recent_transactions(1) {
        Ok(txs) => pass_check(
            "graphql_reachability",
            "GraphQL Reachability",
            format!("connected (sample_txs={})", txs.len()),
            graphql_start,
        ),
        Err(err) => fail_check(
            "graphql_reachability",
            "GraphQL Reachability",
            err.to_string(),
            "Set `SUI_GRAPHQL_ENDPOINT` to a reachable GraphQL endpoint.",
            graphql_start,
        ),
    });

    let walrus_start = Instant::now();
    let walrus = WalrusClient::new(walrus_cache_url.clone(), walrus_aggregator_url.clone());
    checks.push(match walrus.get_latest_checkpoint() {
        Ok(latest) => pass_check(
            "walrus_reachability",
            "Walrus Reachability",
            format!("connected (latest_checkpoint={})", latest),
            walrus_start,
        ),
        Err(err) => fail_check(
            "walrus_reachability",
            "Walrus Reachability",
            err.to_string(),
            "Set `SUI_WALRUS_CACHE_URL` / `SUI_WALRUS_AGGREGATOR_URL` to reachable endpoints.",
            walrus_start,
        ),
    });

    let failed = checks.iter().filter(|c| !c.passed).count();
    let passed = checks.len().saturating_sub(failed);
    Ok(DoctorReport {
        ok: failed == 0,
        passed,
        failed,
        grpc_endpoint,
        graphql_endpoint,
        walrus_cache_url,
        walrus_aggregator_url,
        checks,
    })
}
