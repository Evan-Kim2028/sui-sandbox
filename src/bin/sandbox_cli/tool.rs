use anyhow::{Context, Result};
use clap::Parser;
use serde_json::Value;
use std::path::{Path, PathBuf};

use sui_sandbox_mcp::{state::ProviderConfig, ToolDispatcher};

#[derive(Parser, Debug)]
pub struct ToolCmd {
    /// Tool name (e.g., execute_ptb)
    pub name: String,

    /// JSON input string
    #[arg(long)]
    pub input: Option<String>,

    /// JSON input file path ("-" for stdin)
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Network name for defaults (e.g., mainnet, testnet)
    #[arg(long)]
    pub network: Option<String>,

    /// GraphQL endpoint (defaults to SUI_GRAPHQL_ENDPOINT or network default)
    #[arg(long)]
    pub graphql_url: Option<String>,
}

impl ToolCmd {
    pub async fn execute(
        &self,
        json_output: bool,
        state_file: Option<&Path>,
        rpc_url: &str,
    ) -> Result<()> {
        let input_value = self.read_input()?;
        let dispatcher = ToolDispatcher::new()?;

        let provider_config_path = state_file.and_then(provider_config_path);
        let mut provider_config = provider_config_path
            .as_ref()
            .and_then(|path| load_provider_config(path.as_path()))
            .unwrap_or_else(|| ProviderConfig {
                network: self.network.clone().unwrap_or_default(),
                grpc_endpoint: None,
                graphql_endpoint: None,
            });

        provider_config.grpc_endpoint = Some(rpc_url.to_string());
        if let Some(network) = self.network.as_deref() {
            provider_config.network = network.to_string();
        }
        if let Some(graphql_url) = &self.graphql_url {
            provider_config.graphql_endpoint = Some(graphql_url.to_string());
        } else if provider_config.graphql_endpoint.is_none() {
            provider_config.graphql_endpoint = std::env::var("SUI_GRAPHQL_ENDPOINT").ok();
        }
        provider_config = provider_config.with_defaults();

        dispatcher
            .set_provider_config(provider_config.clone())
            .await;

        if let Some(path) = state_file {
            if path.exists() {
                let mut env_guard = dispatcher.env.lock();
                env_guard
                    .load_state(path)
                    .with_context(|| format!("Failed to load MCP state: {}", path.display()))?;
            }
        }
        let mut response = dispatcher.dispatch(&self.name, input_value).await;
        if let Some(path) = state_file {
            response.state_file = Some(path.to_string_lossy().to_string());
        }

        if response.success {
            if let Some(path) = state_file {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let env_guard = dispatcher.env.lock();
                env_guard
                    .save_state(path)
                    .with_context(|| format!("Failed to save MCP state: {}", path.display()))?;
            }
        }

        if let Some(path) = provider_config_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            save_provider_config(&path, &provider_config)?;
        }

        let output = if json_output || self.pretty {
            serde_json::to_string_pretty(&response)?
        } else {
            serde_json::to_string(&response)?
        };
        println!("{}", output);
        Ok(())
    }

    fn read_input(&self) -> Result<Value> {
        let json_str = if let Some(file) = &self.file {
            if file.as_os_str() == "-" {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(file)
                    .with_context(|| format!("Failed to read file: {}", file.display()))?
            }
        } else if let Some(input) = &self.input {
            input.clone()
        } else {
            return Ok(Value::Object(Default::default()));
        };

        let value: Value =
            serde_json::from_str(&json_str).with_context(|| "Failed to parse JSON input")?;
        Ok(value)
    }
}

fn provider_config_path(state_file: &Path) -> Option<PathBuf> {
    let parent = state_file.parent()?;
    let file = state_file.file_name()?.to_string_lossy();
    Some(parent.join(format!("{}.provider.json", file)))
}

fn load_provider_config(path: &Path) -> Option<ProviderConfig> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_provider_config(path: &Path, config: &ProviderConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)
        .with_context(|| format!("Failed to write provider config: {}", path.display()))
}
