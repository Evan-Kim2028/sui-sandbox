use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::{ensure_system_objects, ReplayCmd};
use sui_sandbox_core::types::is_system_package_address;
use sui_sandbox_core::types::parse_type_tag;
use sui_sandbox_types::{
    normalize_address as normalize_address_shared, FetchedTransaction, GasSummary, PtbArgument,
    PtbCommand, TransactionDigest as SandboxTransactionDigest, TransactionEffectsSummary,
    TransactionInput, TransactionStatus,
};
use sui_state_fetcher::{
    package_data_from_move_package, HistoricalStateProvider, PackageData, ReplayState,
    VersionedObject,
};
use sui_transport::grpc::GrpcOwner;
use sui_types::move_package::MovePackage;
use sui_types::object::{Data as SuiData, Object as SuiObject};
use sui_types::transaction::{
    Argument as SuiArgument, CallArg, Command as SuiCommand, ObjectArg, SharedObjectMutability,
    TransactionData, TransactionDataAPI, TransactionKind,
};
use sui_types::type_input::TypeInput;

impl ReplayCmd {
    fn resolve_igloo_config(&self) -> Result<IglooConfig> {
        let explicit_path = self.igloo.config.clone();
        if let Some(path) = explicit_path.as_ref() {
            if !path.exists() {
                return Err(anyhow!("Igloo config not found: {}", path.display()));
            }
        }
        let config_path = explicit_path
            .or_else(|| {
                std::env::var("IGLOO_MCP_SERVICE_CONFIG")
                    .ok()
                    .map(PathBuf::from)
            })
            .or_else(|| std::env::var("IGLOO_MCP_CONFIG").ok().map(PathBuf::from))
            .or_else(|| find_mcp_service_config(&std::env::current_dir().unwrap_or_default()));

        let mut config = if let Some(path) = config_path {
            if path.exists() {
                load_igloo_config(&path)?
            } else {
                IglooConfig::default()
            }
        } else {
            IglooConfig::default()
        };

        if let Some(cmd) = &self.igloo.command {
            config.command = cmd.clone();
        }

        config.command =
            resolve_igloo_command(&config.command).unwrap_or_else(|| config.command.clone());
        apply_snowflake_config(&mut config);

        Ok(config)
    }

    pub(super) async fn build_replay_state_hybrid(
        &self,
        provider: &HistoricalStateProvider,
        verbose: bool,
    ) -> Result<ReplayState> {
        let mut igloo = IglooMcpClient::connect(self.resolve_igloo_config()?).await?;
        let result = async {
            let digest_sql = escape_sql_literal(&self.digest);

            let meta_query = format!(
                "select CHECKPOINT, EPOCH, TIMESTAMP_MS, EFFECTS_JSON from {}.{}.TRANSACTION where TRANSACTION_DIGEST = '{}' limit 1",
                self.igloo.analytics_db, self.igloo.analytics_schema, digest_sql
            );
            let meta_row = igloo
                .query_one(&meta_query, "hybrid replay: transaction metadata")
                .await
                .context("TRANSACTION metadata query failed")?;
            let checkpoint = row_get_u64(&meta_row, "CHECKPOINT")
                .ok_or_else(|| anyhow!("Missing CHECKPOINT in TRANSACTION for {}", self.digest))?;
            let mut epoch = row_get_u64(&meta_row, "EPOCH").unwrap_or(0);
            let timestamp_ms = row_get_u64(&meta_row, "TIMESTAMP_MS")
                .ok_or_else(|| anyhow!("Missing TIMESTAMP_MS in TRANSACTION for {}", self.digest))?;
            let timestamp_ms_opt = Some(timestamp_ms);
            let effects_raw = row_get_value(&meta_row, "EFFECTS_JSON")
                .ok_or_else(|| anyhow!("Missing EFFECTS_JSON for {}", self.digest))?;
            let effects_json = parse_effects_value(effects_raw)?;
            let shared_versions = parse_effects_versions(&effects_json);
            let effects_summary = build_effects_summary(&effects_json, &shared_versions);
            if epoch == 0 {
                if let Some(executed) = extract_executed_epoch(&effects_json) {
                    epoch = executed;
                }
            }

            let tx_query = format!(
                "select BCS from {}.{}.TRANSACTION_BCS where TRANSACTION_DIGEST = '{}' and TIMESTAMP_MS = {} and CHECKPOINT = {} limit 1",
                self.igloo.analytics_db, self.igloo.analytics_schema, digest_sql, timestamp_ms, checkpoint
            );
            let tx_row = igloo
                .query_one(&tx_query, "hybrid replay: transaction_bcs")
                .await
                .context("TRANSACTION_BCS query failed")?;
            let tx_bcs = row_get_string(&tx_row, "BCS")
                .ok_or_else(|| anyhow!("Missing BCS in TRANSACTION_BCS for {}", self.digest))?;

            let tx_data = decode_transaction_bcs(&tx_bcs)?;
            let ptb = match tx_data.kind() {
                TransactionKind::ProgrammableTransaction(ptb) => ptb,
                other => {
                    return Err(anyhow!(
                        "Hybrid loader only supports programmable transactions (got {:?})",
                        other
                    ))
                }
            };

            let mut input_specs: Vec<InputSpec> = Vec::with_capacity(ptb.inputs.len());
            let mut object_requests: HashMap<AccountAddress, u64> = HashMap::new();
            let mut historical_versions: HashMap<String, u64> = HashMap::new();

            for input in &ptb.inputs {
                match input {
                    CallArg::Pure(bytes) => input_specs.push(InputSpec::Pure(bytes.clone())),
                    CallArg::FundsWithdrawal(_) => {
                        return Err(anyhow!(
                            "Hybrid loader does not support FundsWithdrawal inputs yet"
                        ))
                    }
                    CallArg::Object(obj_arg) => match obj_arg {
                        ObjectArg::ImmOrOwnedObject(obj_ref) => {
                            let addr = AccountAddress::from(obj_ref.0);
                            let version = obj_ref.1.value();
                            let digest = obj_ref.2.to_string();
                            input_specs.push(InputSpec::ImmOrOwned {
                                id: addr,
                                version,
                                digest,
                            });
                            object_requests.insert(addr, version);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), version);
                        }
                        ObjectArg::Receiving(obj_ref) => {
                            let addr = AccountAddress::from(obj_ref.0);
                            let version = obj_ref.1.value();
                            let digest = obj_ref.2.to_string();
                            input_specs.push(InputSpec::Receiving {
                                id: addr,
                                version,
                                digest,
                            });
                            object_requests.insert(addr, version);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), version);
                        }
                        ObjectArg::SharedObject {
                            id,
                            initial_shared_version,
                            mutability,
                        } => {
                            let addr = AccountAddress::from(*id);
                            let initial = initial_shared_version.value();
                            let normalized = normalize_address_shared(&addr.to_hex_literal());
                            let actual = shared_versions.get(&normalized).copied().unwrap_or(initial);
                            let mutable = matches!(mutability, SharedObjectMutability::Mutable);
                            input_specs.push(InputSpec::Shared {
                                id: addr,
                                initial_shared_version: initial,
                                mutable,
                            });
                            object_requests.insert(addr, actual);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), actual);
                        }
                    },
                }
            }

            if verbose {
                eprintln!(
                    "[hybrid] inputs={} object_requests={}",
                    input_specs.len(),
                    object_requests.len()
                );
            }

            let mut objects: HashMap<AccountAddress, VersionedObject> = HashMap::new();
            let mut owner_map: HashMap<AccountAddress, GrpcOwner> = HashMap::new();
            for (addr, version) in object_requests {
                let id_hex = addr.to_hex_literal();
                if verbose {
                    eprintln!("[hybrid] fetch object {} @{}", id_hex, version);
                }
                let grpc_obj = match tokio::time::timeout(
                    Duration::from_secs(self.grpc_timeout_secs),
                    provider.grpc().get_object_at_version(&id_hex, Some(version)),
                )
                .await
                {
                    Ok(result) => result?
                        .ok_or_else(|| anyhow!("Object not found: {} @{}", id_hex, version))?,
                    Err(_) => {
                        return Err(anyhow!(
                            "gRPC timeout fetching object {} @{} ({}s)",
                            id_hex,
                            version,
                            self.grpc_timeout_secs
                        ))
                    }
                };
                if verbose {
                    eprintln!("[hybrid] fetched object {} @{}", id_hex, version);
                }
                let versioned = grpc_object_to_versioned(&grpc_obj, addr, version)?;
                owner_map.insert(addr, grpc_obj.owner.clone());
                objects.insert(addr, versioned);
            }

            if self.hydration.auto_system_objects {
                ensure_system_objects(
                    &mut objects,
                    &historical_versions,
                    timestamp_ms_opt,
                    Some(checkpoint),
                );
            }

            let inputs = build_transaction_inputs(&input_specs, &owner_map);
            let commands = convert_sui_commands(&ptb.commands)?;

            let sender = AccountAddress::from(tx_data.sender());
            let transaction = FetchedTransaction {
                digest: SandboxTransactionDigest(self.digest.clone()),
                sender,
                gas_budget: tx_data.gas_budget(),
                gas_price: tx_data.gas_price(),
                commands,
                inputs,
                effects: effects_summary,
                timestamp_ms: timestamp_ms_opt,
                checkpoint: Some(checkpoint),
            };

            let mut package_ids = collect_package_ids_from_commands(&ptb.commands);
            for obj in objects.values() {
                if let Some(type_tag) = &obj.type_tag {
                    collect_package_ids_from_type_str(type_tag, &mut package_ids);
                }
            }
            if verbose {
                eprintln!("[hybrid] package seeds={}", package_ids.len());
            }

            let mut packages: HashMap<AccountAddress, PackageData> = HashMap::new();
            let mut pending: VecDeque<AccountAddress> = package_ids.into_iter().collect();
            let mut seen: HashSet<AccountAddress> = HashSet::new();
            let mut have_storage: HashSet<AccountAddress> = HashSet::new();
            let mut have_runtime: HashSet<AccountAddress> = HashSet::new();

            while let Some(pkg_id) = pending.pop_front() {
                if !seen.insert(pkg_id) {
                    continue;
                }
                if have_storage.contains(&pkg_id) || have_runtime.contains(&pkg_id) {
                    continue;
                }

                if verbose {
                    eprintln!("[hybrid] fetch package {}", pkg_id.to_hex_literal());
                }
                let is_system_pkg = is_system_package_address(&pkg_id);
                if is_system_pkg && verbose {
                    eprintln!(
                        "[hybrid] system package -> gRPC {}",
                        pkg_id.to_hex_literal()
                    );
                }
                let mut pkg_opt = None;
                if self.igloo.snowflake_packages && !is_system_pkg {
                    pkg_opt = fetch_package_from_snowflake(
                        &mut igloo,
                        &self.igloo.analytics_db,
                        &self.igloo.analytics_schema,
                        &pkg_id,
                        Some(checkpoint),
                        timestamp_ms_opt,
                    )
                    .await?;
                }
                if pkg_opt.is_none() && self.igloo.require_snowflake_packages && !is_system_pkg {
                    return Err(anyhow!(
                        "Snowflake package missing for {}",
                        pkg_id.to_hex_literal()
                    ));
                }
                if pkg_opt.is_none() {
                    pkg_opt = fetch_package_via_grpc(provider, &pkg_id, None).await?;
                }

                if let Some(pkg) = pkg_opt {
                    if verbose {
                        eprintln!(
                            "[hybrid] fetched package {} (modules={})",
                            pkg.address.to_hex_literal(),
                            pkg.modules.len()
                        );
                    }
                    let deps = extract_module_dependency_ids(&pkg.modules);
                    for dep in deps {
                        if !seen.contains(&dep)
                            && !have_storage.contains(&dep)
                            && !have_runtime.contains(&dep)
                        {
                            pending.push_back(dep);
                        }
                    }
                    have_storage.insert(pkg.address);
                    have_runtime.insert(pkg.runtime_id());
                    packages.insert(pkg.address, pkg);
                } else if verbose {
                    eprintln!("[hybrid] missing package {}", pkg_id.to_hex_literal());
                }
            }

            let mut protocol_version = 0u64;
            let mut reference_gas_price = None;
            if epoch > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(self.grpc_timeout_secs),
                    provider.grpc().get_epoch(Some(epoch)),
                )
                .await
                {
                    Ok(Ok(Some(ep))) => {
                        protocol_version = ep.protocol_version.unwrap_or(0);
                        reference_gas_price = ep.reference_gas_price;
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        if verbose {
                            eprintln!(
                                "[hybrid] get_epoch failed for epoch {}: {}",
                                epoch, err
                            );
                        }
                    }
                    Err(_) => {
                        if verbose {
                            eprintln!(
                                "[hybrid] get_epoch timeout for epoch {} ({}s)",
                                epoch, self.grpc_timeout_secs
                            );
                        }
                    }
                }
            }

            Ok(ReplayState {
                transaction,
                objects,
                packages,
                protocol_version,
                epoch,
                reference_gas_price,
                checkpoint: Some(checkpoint),
            })
        }
        .await;

        let _ = igloo.shutdown().await;
        result
    }
}
#[derive(Debug, Clone)]
enum InputSpec {
    Pure(Vec<u8>),
    ImmOrOwned {
        id: AccountAddress,
        version: u64,
        digest: String,
    },
    Receiving {
        id: AccountAddress,
        version: u64,
        digest: String,
    },
    Shared {
        id: AccountAddress,
        initial_shared_version: u64,
        mutable: bool,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
struct IglooConfigFile {
    igloo: Option<IglooConfigSection>,
    snowflake: Option<SnowflakeConfigSection>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct IglooConfigSection {
    command: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SnowflakeConfigSection {
    account: Option<String>,
    user: Option<String>,
    database: Option<String>,
    schema: Option<String>,
    warehouse: Option<String>,
    role: Option<String>,
    authenticator: Option<String>,
}

#[derive(Debug, Clone)]
struct IglooConfig {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
    snowflake: Option<SnowflakeConfigSection>,
}

impl Default for IglooConfig {
    fn default() -> Self {
        Self {
            command: "igloo_mcp".to_string(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            snowflake: None,
        }
    }
}

struct IglooMcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

const IGLOO_QUERY_TIMEOUT_SECS: u64 = 120;

impl IglooMcpClient {
    async fn connect(config: IglooConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().context("Failed to spawn igloo-mcp")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open igloo-mcp stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open igloo-mcp stdout"))?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "sui-sandbox",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }
        });
        self.send_message(&init).await?;
        let _ = self.read_response(id).await?;

        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        self.send_message(&initialized).await?;
        Ok(())
    }

    async fn query_one(&mut self, statement: &str, reason: &str) -> Result<Value> {
        let rows = self.query_rows(statement, reason).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No rows returned for query"))
    }

    async fn query_rows(&mut self, statement: &str, reason: &str) -> Result<Vec<Value>> {
        let payload = serde_json::json!({
            "statement": statement,
            "reason": reason,
            "result_mode": "full",
            "timeout_seconds": IGLOO_QUERY_TIMEOUT_SECS,
        });
        let result = self.call_tool("execute_query", payload).await?;
        let mut structured = result
            .get("structuredContent")
            .cloned()
            .unwrap_or(result.clone());
        if let Some(inner) = structured.get("result") {
            structured = inner.clone();
        }
        let rows = structured
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            }
        });
        self.send_message(&request).await?;
        let result = self.read_response(id).await?;
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let message = extract_text_from_content(result.get("content"))
                .unwrap_or_else(|| "igloo-mcp tool error".to_string());
            return Err(anyhow!("igloo-mcp {} failed: {}", name, message));
        }
        Ok(result)
    }

    async fn send_message(&mut self, value: &Value) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_response(&mut self, expected_id: u64) -> Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.stdout.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(anyhow!("igloo-mcp closed stdout unexpectedly"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Value = match serde_json::from_str(trimmed) {
                Ok(val) => val,
                Err(_) => continue,
            };
            let Some(id_value) = parsed.get("id") else {
                continue;
            };
            if !json_id_matches(id_value, expected_id) {
                continue;
            }
            if let Some(err) = parsed.get("error") {
                return Err(anyhow!("igloo-mcp error: {}", err));
            }
            return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.stdin.shutdown().await;
        let wait = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        if wait.is_err() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        Ok(())
    }
}

fn find_mcp_service_config(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors().take(6) {
        let candidate = ancestor.join("mcp_service_config.json");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn load_igloo_config(path: &Path) -> Result<IglooConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read igloo config: {}", path.display()))?;
    let parsed: IglooConfigFile =
        serde_json::from_str(&raw).context("Failed to parse igloo config JSON")?;
    let igloo = parsed.igloo.unwrap_or_default();
    Ok(IglooConfig {
        command: igloo.command.unwrap_or_else(|| "igloo_mcp".to_string()),
        args: igloo.args.unwrap_or_default(),
        cwd: igloo.cwd.map(PathBuf::from),
        env: igloo.env.unwrap_or_default(),
        snowflake: parsed.snowflake,
    })
}

fn resolve_igloo_command(command: &str) -> Option<String> {
    let path = Path::new(command);
    if command.contains('/') || path.is_absolute() {
        if path.exists() {
            return Some(command.to_string());
        }
        if command.ends_with("igloo-mcp") {
            let alt = command.replace("igloo-mcp", "igloo_mcp");
            if Path::new(&alt).exists() {
                return Some(alt);
            }
        }
        return find_in_path("igloo_mcp");
    }
    find_in_path(command).or_else(|| find_in_path("igloo_mcp"))
}

fn find_in_path(command: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(command);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn apply_snowflake_config(config: &mut IglooConfig) {
    let Some(sf) = config.snowflake.as_ref() else {
        return;
    };
    let has_profile = has_arg(&config.args, &["--profile"])
        || config.env.contains_key("SNOWFLAKE_PROFILE")
        || config.env.contains_key("SNOWCLI_DEFAULT_PROFILE");
    if has_profile {
        strip_login_env(&mut config.env);
        set_env_if_missing(&mut config.env, "SNOWFLAKE_DATABASE", sf.database.as_ref());
        set_env_if_missing(&mut config.env, "SNOWFLAKE_SCHEMA", sf.schema.as_ref());
        return;
    }

    push_arg_if_missing(
        &mut config.args,
        &["--account", "--account-identifier"],
        sf.account.as_deref(),
    );
    push_arg_if_missing(
        &mut config.args,
        &["--user", "--username"],
        sf.user.as_deref(),
    );
    push_arg_if_missing(&mut config.args, &["--warehouse"], sf.warehouse.as_deref());
    push_arg_if_missing(&mut config.args, &["--role"], sf.role.as_deref());
    push_arg_if_missing(
        &mut config.args,
        &["--authenticator"],
        sf.authenticator.as_deref(),
    );

    set_env_if_missing(&mut config.env, "SNOWFLAKE_ACCOUNT", sf.account.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_USER", sf.user.as_ref());
    set_env_if_missing(
        &mut config.env,
        "SNOWFLAKE_AUTHENTICATOR",
        sf.authenticator.as_ref(),
    );
    set_env_if_missing(&mut config.env, "SNOWFLAKE_DATABASE", sf.database.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_SCHEMA", sf.schema.as_ref());
    set_env_if_missing(
        &mut config.env,
        "SNOWFLAKE_WAREHOUSE",
        sf.warehouse.as_ref(),
    );
    set_env_if_missing(&mut config.env, "SNOWFLAKE_ROLE", sf.role.as_ref());
}

fn push_arg_if_missing(args: &mut Vec<String>, names: &[&str], value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    if has_arg(args, names) {
        return;
    }
    args.push(names[0].to_string());
    args.push(value.to_string());
}

fn has_arg(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|arg| {
        names.iter().any(|name| {
            if arg == name {
                return true;
            }
            let prefix = format!("{}=", name);
            arg.starts_with(&prefix)
        })
    })
}

fn strip_login_env(env: &mut HashMap<String, String>) {
    for key in [
        "SNOWFLAKE_ACCOUNT",
        "SNOWFLAKE_USER",
        "SNOWFLAKE_PASSWORD",
        "SNOWFLAKE_PAT",
        "SNOWFLAKE_ROLE",
        "SNOWFLAKE_WAREHOUSE",
        "SNOWFLAKE_PASSCODE",
        "SNOWFLAKE_PASSCODE_IN_PASSWORD",
        "SNOWFLAKE_PRIVATE_KEY",
        "SNOWFLAKE_PRIVATE_KEY_FILE",
        "SNOWFLAKE_PRIVATE_KEY_FILE_PWD",
        "SNOWFLAKE_AUTHENTICATOR",
        "SNOWFLAKE_HOST",
    ] {
        env.remove(key);
    }
}

fn set_env_if_missing(env: &mut HashMap<String, String>, key: &str, value: Option<&String>) {
    if env.contains_key(key) {
        return;
    }
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    env.insert(key.to_string(), value.clone());
}

fn json_id_matches(value: &Value, expected: u64) -> bool {
    match value {
        Value::Number(num) => num.as_u64() == Some(expected),
        Value::String(s) => s.parse::<u64>().ok() == Some(expected),
        _ => false,
    }
}

fn extract_text_from_content(value: Option<&Value>) -> Option<String> {
    let content = value?.as_array()?;
    for item in content {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }
    None
}

fn escape_sql_literal(input: &str) -> String {
    input.replace('\'', "''")
}

fn row_get_value<'a>(row: &'a Value, key: &str) -> Option<&'a Value> {
    let obj = row.as_object()?;
    if let Some(val) = obj.get(key) {
        return Some(val);
    }
    let upper = key.to_ascii_uppercase();
    if let Some(val) = obj.get(&upper) {
        return Some(val);
    }
    let lower = key.to_ascii_lowercase();
    obj.get(&lower)
}

fn row_get_string(row: &Value, key: &str) -> Option<String> {
    row_get_value(row, key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn row_get_u64(row: &Value, key: &str) -> Option<u64> {
    row_get_value(row, key).and_then(value_as_u64)
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(num) => num.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_effects_value(value: &Value) -> Result<Value> {
    match value {
        Value::String(text) => serde_json::from_str(text).context("Failed to parse EFFECTS_JSON"),
        Value::Object(_) | Value::Array(_) => Ok(value.clone()),
        _ => Err(anyhow!("Unexpected EFFECTS_JSON format")),
    }
}

fn parse_effects_versions(effects: &Value) -> HashMap<String, u64> {
    let mut versions = HashMap::new();
    let root = effects.get("V2").unwrap_or(effects);

    if let Some(changed) = root.get("changed_objects").and_then(|v| v.as_array()) {
        for entry in changed {
            let id = entry.get(0).and_then(|v| v.as_str());
            let Some(id_str) = id else { continue };
            let input_state = entry.get(1).and_then(|v| v.get("input_state"));
            if let Some(version) = input_state.and_then(extract_version_from_input_state) {
                versions.insert(normalize_address_shared(id_str), version);
            }
        }
    }

    if let Some(unchanged) = root
        .get("unchanged_consensus_objects")
        .and_then(|v| v.as_array())
    {
        for entry in unchanged {
            let id = entry.get(0).and_then(|v| v.as_str());
            let Some(id_str) = id else { continue };
            let info = entry.get(1);
            let Some(info) = info else { continue };
            let version = info
                .get("ReadOnlyRoot")
                .or_else(|| info.get("ReadOnly"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(value_as_u64);
            if let Some(ver) = version {
                versions.insert(normalize_address_shared(id_str), ver);
            }
        }
    }

    versions
}

fn extract_version_from_input_state(input_state: &Value) -> Option<u64> {
    let exist = input_state.get("Exist")?;
    let arr = exist.as_array()?;
    let version_entry = arr.first()?.as_array()?;
    value_as_u64(version_entry.first()?)
}

fn extract_executed_epoch(effects: &Value) -> Option<u64> {
    let root = effects.get("V2").unwrap_or(effects);
    root.get("executed_epoch").and_then(value_as_u64)
}

fn build_effects_summary(
    effects: &Value,
    shared_versions: &HashMap<String, u64>,
) -> Option<TransactionEffectsSummary> {
    let root = effects.get("V2").unwrap_or(effects);
    let status_value = root.get("status")?;
    let status = match status_value {
        Value::String(s) => {
            if s.eq_ignore_ascii_case("success") {
                TransactionStatus::Success
            } else {
                TransactionStatus::Failure { error: s.clone() }
            }
        }
        Value::Object(map) => {
            if map.contains_key("Success") {
                TransactionStatus::Success
            } else if let Some(err) = map
                .get("Failure")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                TransactionStatus::Failure { error: err }
            } else {
                TransactionStatus::Failure {
                    error: "Unknown failure".to_string(),
                }
            }
        }
        _ => TransactionStatus::Failure {
            error: "Unknown failure".to_string(),
        },
    };

    let mut created = Vec::new();
    let mut mutated = Vec::new();
    let mut deleted = Vec::new();

    if let Some(changed) = root.get("changed_objects").and_then(|v| v.as_array()) {
        for entry in changed {
            let id = entry.get(0).and_then(|v| v.as_str()).map(|s| s.to_string());
            let Some(id) = id else { continue };
            let op = entry
                .get(1)
                .and_then(|v| v.get("id_operation"))
                .and_then(|v| v.as_str())
                .unwrap_or("None");
            match op {
                "Created" => created.push(id),
                "Deleted" => deleted.push(id),
                _ => mutated.push(id),
            }
        }
    }

    Some(TransactionEffectsSummary {
        status,
        created,
        mutated,
        deleted,
        wrapped: Vec::new(),
        unwrapped: Vec::new(),
        gas_used: GasSummary::default(),
        events_count: 0,
        shared_object_versions: shared_versions.clone(),
    })
}

fn decode_transaction_bcs(bcs_str: &str) -> Result<TransactionData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bcs_str)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(bcs_str))
        .context("Failed to decode transaction BCS")?;
    let tx: TransactionData = bcs::from_bytes(&bytes).context("Failed to parse transaction BCS")?;
    Ok(tx)
}

fn convert_sui_commands(commands: &[SuiCommand]) -> Result<Vec<PtbCommand>> {
    commands.iter().map(convert_sui_command).collect()
}

fn convert_sui_command(command: &SuiCommand) -> Result<PtbCommand> {
    Ok(match command {
        SuiCommand::MoveCall(call) => PtbCommand::MoveCall {
            package: format!("{}", call.package),
            module: call.module.clone(),
            function: call.function.clone(),
            type_arguments: call
                .type_arguments
                .iter()
                .map(|t| t.to_canonical_string(true))
                .collect(),
            arguments: call.arguments.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::TransferObjects(objs, addr) => PtbCommand::TransferObjects {
            objects: objs.iter().map(convert_sui_argument).collect(),
            address: convert_sui_argument(addr),
        },
        SuiCommand::SplitCoins(coin, amounts) => PtbCommand::SplitCoins {
            coin: convert_sui_argument(coin),
            amounts: amounts.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MergeCoins(dest, sources) => PtbCommand::MergeCoins {
            destination: convert_sui_argument(dest),
            sources: sources.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MakeMoveVec(type_arg, elems) => PtbCommand::MakeMoveVec {
            type_arg: type_arg.as_ref().map(|t| t.to_canonical_string(true)),
            elements: elems.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::Publish(modules, deps) => PtbCommand::Publish {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            dependencies: deps.iter().map(|d| format!("{}", d)).collect(),
        },
        SuiCommand::Upgrade(modules, _deps, package, ticket) => PtbCommand::Upgrade {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            package: format!("{}", package),
            ticket: convert_sui_argument(ticket),
        },
    })
}

fn convert_sui_argument(arg: &SuiArgument) -> PtbArgument {
    match arg {
        SuiArgument::GasCoin => PtbArgument::GasCoin,
        SuiArgument::Input(index) => PtbArgument::Input { index: *index },
        SuiArgument::Result(index) => PtbArgument::Result { index: *index },
        SuiArgument::NestedResult(index, result_index) => PtbArgument::NestedResult {
            index: *index,
            result_index: *result_index,
        },
    }
}

fn build_transaction_inputs(
    specs: &[InputSpec],
    owner_map: &HashMap<AccountAddress, GrpcOwner>,
) -> Vec<TransactionInput> {
    specs
        .iter()
        .map(|spec| match spec {
            InputSpec::Pure(bytes) => TransactionInput::Pure {
                bytes: bytes.clone(),
            },
            InputSpec::ImmOrOwned {
                id,
                version,
                digest,
            } => {
                let is_immutable = matches!(owner_map.get(id), Some(GrpcOwner::Immutable));
                if is_immutable {
                    TransactionInput::ImmutableObject {
                        object_id: id.to_hex_literal(),
                        version: *version,
                        digest: digest.clone(),
                    }
                } else {
                    TransactionInput::Object {
                        object_id: id.to_hex_literal(),
                        version: *version,
                        digest: digest.clone(),
                    }
                }
            }
            InputSpec::Receiving {
                id,
                version,
                digest,
            } => TransactionInput::Receiving {
                object_id: id.to_hex_literal(),
                version: *version,
                digest: digest.clone(),
            },
            InputSpec::Shared {
                id,
                initial_shared_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: id.to_hex_literal(),
                initial_shared_version: *initial_shared_version,
                mutable: *mutable,
            },
        })
        .collect()
}

fn collect_package_ids_from_commands(commands: &[SuiCommand]) -> HashSet<AccountAddress> {
    let mut packages = HashSet::new();
    for cmd in commands {
        match cmd {
            SuiCommand::MoveCall(call) => {
                packages.insert(AccountAddress::from(call.package));
                for ty in &call.type_arguments {
                    collect_packages_from_type_input(ty, &mut packages);
                }
            }
            SuiCommand::Publish(_, deps) => {
                for dep in deps {
                    packages.insert(AccountAddress::from(*dep));
                }
            }
            SuiCommand::Upgrade(_, deps, package, _) => {
                packages.insert(AccountAddress::from(*package));
                for dep in deps {
                    packages.insert(AccountAddress::from(*dep));
                }
            }
            SuiCommand::MakeMoveVec(Some(tag), _) => {
                collect_packages_from_type_input(tag, &mut packages);
            }
            _ => {}
        }
    }
    packages
}

fn collect_packages_from_type_input(input: &TypeInput, out: &mut HashSet<AccountAddress>) {
    match input {
        TypeInput::Struct(s) => {
            out.insert(s.address);
            for ty in &s.type_params {
                collect_packages_from_type_input(ty, out);
            }
        }
        TypeInput::Vector(inner) => collect_packages_from_type_input(inner, out),
        _ => {}
    }
}

fn collect_package_ids_from_type_str(type_str: &str, out: &mut HashSet<AccountAddress>) {
    if let Ok(tag) = parse_type_tag(type_str) {
        collect_package_ids_from_type_tag(&tag, out);
    }
}

fn collect_package_ids_from_type_tag(tag: &TypeTag, out: &mut HashSet<AccountAddress>) {
    match tag {
        TypeTag::Struct(s) => {
            out.insert(s.address);
            for ty in &s.type_params {
                collect_package_ids_from_type_tag(ty, out);
            }
        }
        TypeTag::Vector(inner) => collect_package_ids_from_type_tag(inner, out),
        _ => {}
    }
}

async fn fetch_package_from_snowflake(
    igloo: &mut IglooMcpClient,
    database: &str,
    schema: &str,
    package_id: &AccountAddress,
    checkpoint: Option<u64>,
    timestamp_ms: Option<u64>,
) -> Result<Option<PackageData>> {
    let pkg_hex = package_id.to_hex_literal();
    let mut statement = format!(
        "select BCS, CHECKPOINT from {}.{}.MOVE_PACKAGE_BCS where PACKAGE_ID = '{}'",
        database,
        schema,
        escape_sql_literal(&pkg_hex)
    );
    if let Some(ts) = timestamp_ms {
        statement.push_str(&format!(" and TIMESTAMP_MS <= {}", ts));
    }
    if let Some(cp) = checkpoint {
        statement.push_str(&format!(" and CHECKPOINT <= {}", cp));
    }
    if timestamp_ms.is_some() {
        statement.push_str(" order by TIMESTAMP_MS desc limit 1");
    } else {
        statement.push_str(" order by CHECKPOINT desc limit 1");
    }
    let rows = igloo
        .query_rows(&statement, "hybrid replay: package_bcs")
        .await?;
    let row = match rows.first() {
        Some(row) => row,
        None => return Ok(None),
    };
    let bcs = match row_get_string(row, "BCS") {
        Some(v) => v,
        None => return Ok(None),
    };
    let pkg = decode_move_package_bcs(&bcs)?;
    Ok(Some(pkg))
}

async fn fetch_package_via_grpc(
    provider: &HistoricalStateProvider,
    package_id: &AccountAddress,
    version: Option<u64>,
) -> Result<Option<PackageData>> {
    let pkg_hex = package_id.to_hex_literal();
    let obj = provider
        .grpc()
        .get_object_at_version(&pkg_hex, version)
        .await?;
    let Some(obj) = obj else { return Ok(None) };
    if obj.package_modules.is_none() {
        return Ok(None);
    }
    let pkg = grpc_object_to_package_data(&obj, *package_id)?;
    Ok(Some(pkg))
}

fn decode_move_package_bcs(bcs_str: &str) -> Result<PackageData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bcs_str)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(bcs_str))
        .context("Failed to decode package BCS")?;
    if let Ok(obj) = bcs::from_bytes::<SuiObject>(&bytes) {
        if let SuiData::Package(pkg) = &obj.data {
            return Ok(package_data_from_move_package(pkg));
        }
    }
    let pkg: MovePackage = bcs::from_bytes(&bytes).context("Failed to parse package BCS")?;
    Ok(package_data_from_move_package(&pkg))
}

fn grpc_object_to_versioned(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    id: AccountAddress,
    version: u64,
) -> Result<VersionedObject> {
    let (is_shared, is_immutable) = match &grpc_obj.owner {
        GrpcOwner::Shared { .. } => (true, false),
        GrpcOwner::Immutable => (false, true),
        _ => (false, false),
    };
    Ok(VersionedObject {
        id,
        version,
        digest: Some(grpc_obj.digest.clone()),
        type_tag: grpc_obj.type_string.clone(),
        bcs_bytes: grpc_obj.bcs.clone().unwrap_or_default(),
        is_shared,
        is_immutable,
    })
}

fn grpc_object_to_package_data(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    address: AccountAddress,
) -> Result<PackageData> {
    let modules = grpc_obj.package_modules.clone().unwrap_or_default();
    let mut linkage = HashMap::new();
    if let Some(entries) = &grpc_obj.package_linkage {
        for entry in entries {
            if let (Ok(orig), Ok(upg)) = (
                AccountAddress::from_hex_literal(&entry.original_id),
                AccountAddress::from_hex_literal(&entry.upgraded_id),
            ) {
                linkage.insert(orig, upg);
            }
        }
    }
    let original_id = grpc_obj
        .package_original_id
        .as_ref()
        .and_then(|s| AccountAddress::from_hex_literal(s).ok());
    Ok(PackageData {
        address,
        version: grpc_obj.version,
        modules,
        linkage,
        original_id,
    })
}

fn extract_module_dependency_ids(modules: &[(String, Vec<u8>)]) -> Vec<AccountAddress> {
    let mut deps: HashSet<AccountAddress> = HashSet::new();
    for (_, bytes) in modules {
        if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
            for dep in module.immediate_dependencies() {
                deps.insert(*dep.address());
            }
        }
    }
    deps.into_iter().collect()
}
