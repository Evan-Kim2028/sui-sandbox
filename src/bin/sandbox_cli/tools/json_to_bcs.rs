//! Convert JSON object data to BCS bytes using Move bytecode layouts.

use anyhow::{Context, Result};
use base64::Engine;
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

use sui_sandbox_core::utilities::JsonToBcsConverter;

#[derive(Debug, Parser)]
#[command(
    name = "json-to-bcs",
    about = "Convert a JSON object to BCS bytes using Move bytecode struct layouts"
)]
pub struct JsonToBcsCmd {
    /// Full Move type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
    #[arg(long, value_name = "TYPE")]
    pub r#type: String,

    /// Path to JSON file (use "-" for stdin)
    #[arg(long, value_name = "FILE")]
    pub json_file: PathBuf,

    /// Directory containing bytecode_modules/*.mv files
    #[arg(long, value_name = "DIR")]
    pub bytecode_dir: PathBuf,
}

#[derive(Debug, Serialize)]
struct JsonToBcsResult {
    bcs_base64: String,
    r#type: String,
    size_bytes: usize,
}

impl JsonToBcsCmd {
    pub fn execute(&self, json_output: bool) -> Result<()> {
        // Read JSON input
        let json_str = if self.json_file.as_os_str() == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("read stdin")?;
            buf
        } else {
            std::fs::read_to_string(&self.json_file)
                .with_context(|| format!("read {}", self.json_file.display()))?
        };

        let json_value: serde_json::Value =
            serde_json::from_str(&json_str).context("parse JSON input")?;

        // Load bytecode modules
        let bytecode_dir = self.bytecode_dir.join("bytecode_modules");
        let dir = if bytecode_dir.is_dir() {
            &bytecode_dir
        } else {
            // Allow pointing directly at a directory of .mv files
            &self.bytecode_dir
        };

        let mut bytecode_list = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("read {}", dir.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("list {}", dir.display()))?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("mv") {
                let bytes =
                    std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                bytecode_list.push(bytes);
            }
        }

        if bytecode_list.is_empty() {
            anyhow::bail!(
                "no .mv files found in {}",
                dir.display()
            );
        }

        // Convert
        let mut converter = JsonToBcsConverter::new();
        converter.add_modules_from_bytes(&bytecode_list)?;
        let bcs_bytes = converter.convert(&self.r#type, &json_value)?;

        if json_output {
            let result = JsonToBcsResult {
                bcs_base64: base64::engine::general_purpose::STANDARD.encode(&bcs_bytes),
                r#type: self.r#type.clone(),
                size_bytes: bcs_bytes.len(),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            // Raw base64 to stdout for piping
            println!(
                "{}",
                base64::engine::general_purpose::STANDARD.encode(&bcs_bytes)
            );
        }

        Ok(())
    }
}
