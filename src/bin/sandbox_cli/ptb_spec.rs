//! Shared PTB spec parsing for CLI commands.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

/// Read a PTB spec JSON value from a file (or stdin if enabled).
pub fn read_ptb_spec_value(path: &Path, allow_stdin: bool) -> Result<Value> {
    let json_str = if allow_stdin && path.as_os_str() == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read PTB spec: {}", path.display()))?
    };

    serde_json::from_str(&json_str).context("Failed to parse PTB spec JSON")
}

/// Parse a CLI PTB spec, rejecting legacy formats.
pub fn parse_ptb_spec_value(value: Value) -> Result<PtbSpec> {
    ensure_no_legacy_commands(&value)?;
    serde_json::from_value(value).context("Failed to parse CLI PTB spec")
}

/// Read and parse a CLI PTB spec from a path (or stdin if enabled).
pub fn read_ptb_spec(path: &Path, allow_stdin: bool) -> Result<PtbSpec> {
    let value = read_ptb_spec_value(path, allow_stdin)?;
    parse_ptb_spec_value(value)
}

fn ensure_no_legacy_commands(value: &Value) -> Result<()> {
    if value.get("commands").is_some() {
        return Err(anyhow!(
            "Legacy PTB specs with a 'commands' field are no longer supported. Use the CLI PTB spec with a 'calls' array."
        ));
    }
    Ok(())
}

/// JSON spec format for PTB
#[derive(Debug, Deserialize)]
pub struct PtbSpec {
    /// Input values
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// Commands to execute
    pub calls: Vec<CallSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Pure(PureInput),
    Object(ObjectInputSpec),
}

#[derive(Debug, Deserialize)]
pub struct PureInput {
    #[serde(flatten)]
    pub value: PureValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PureValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    Bool(bool),
    Address(String),
    #[serde(rename = "vector_u8_utf8")]
    VectorU8Utf8(String),
    #[serde(rename = "vector_u8_hex")]
    VectorU8Hex(String),
    #[serde(rename = "vector_address")]
    VectorAddress(Vec<String>),
    #[serde(rename = "vector_u64")]
    VectorU64(Vec<u64>),
}

#[derive(Debug, Deserialize)]
pub struct ObjectInputSpec {
    #[serde(rename = "imm_or_owned_object")]
    pub imm_or_owned: Option<String>,
    #[serde(rename = "shared_object")]
    pub shared: Option<SharedObjectSpec>,
}

#[derive(Debug, Deserialize)]
pub struct SharedObjectSpec {
    pub id: String,
    pub mutable: bool,
}

#[derive(Debug, Deserialize)]
pub struct CallSpec {
    /// Target: "0xADDR::module::function"
    pub target: String,
    /// Type arguments
    #[serde(default, alias = "type_arguments")]
    pub type_args: Vec<String>,
    /// Arguments (references to inputs or results)
    #[serde(default, alias = "arguments")]
    pub args: Vec<ArgSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ArgSpec {
    Inline(InlineArg),
    Reference(ArgReference),
}

#[derive(Debug, Deserialize)]
pub struct InlineArg {
    #[serde(flatten)]
    pub value: PureValue,
}

#[derive(Debug, Deserialize)]
pub struct ArgReference {
    /// Input index
    #[serde(alias = "Input")]
    pub input: Option<u16>,
    /// Result index from previous command
    #[serde(alias = "Result")]
    pub result: Option<u16>,
    /// Nested result [cmd_index, result_index]
    #[serde(alias = "NestedResult")]
    pub nested_result: Option<[u16; 2]>,
    /// Gas coin reference
    #[serde(alias = "GasCoin")]
    pub gas_coin: Option<bool>,
}
