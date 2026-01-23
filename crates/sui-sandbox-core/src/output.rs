//! # OutputFormatter - Unified Output Layer
//!
//! This module provides a consistent output formatting abstraction across all subsystems.
//! Instead of each command implementing its own output logic, they all use `OutputFormatter`
//! to produce JSON, JSONL, CSV, or human-readable output.
//!
//! ## Usage
//!
//! ```
//! use sui_sandbox_core::output::{OutputFormatter, OutputFormat};
//!
//! let formatter = OutputFormatter::new(OutputFormat::Json);
//! let results = vec!["hello", "world"];
//! let output = formatter.format_value(&results).unwrap();
//! assert!(output.contains("hello"));
//! ```

use anyhow::Result;
use serde::Serialize;
use std::io::Write;

/// Output format options supported by all subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Pretty-printed JSON (default for single results)
    #[default]
    Json,
    /// JSON Lines format (one JSON object per line, for streaming)
    JsonLines,
    /// Comma-separated values (for spreadsheet compatibility)
    Csv,
    /// Human-readable text format
    Human,
}

impl OutputFormat {
    /// Parse format from string (case-insensitive).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "json" => Some(Self::Json),
            "jsonl" | "jsonlines" | "ndjson" => Some(Self::JsonLines),
            "csv" => Some(Self::Csv),
            "human" | "text" | "readable" => Some(Self::Human),
            _ => None,
        }
    }

    /// Get the file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::JsonLines => "jsonl",
            Self::Csv => "csv",
            Self::Human => "txt",
        }
    }

    /// Get the MIME type for this format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::JsonLines => "application/x-ndjson",
            Self::Csv => "text/csv",
            Self::Human => "text/plain",
        }
    }
}

/// Unified output formatter for all subsystems.
///
/// This provides consistent formatting across:
/// - Single package mode
/// - Batch mode
/// - Corpus mode
/// - Benchmark results
/// - TX replay results
pub struct OutputFormatter {
    format: OutputFormat,
    /// Whether to include metadata (timestamps, versions) in output
    include_metadata: bool,
    /// Pretty-print JSON (default: true for Json, false for JsonLines)
    pretty: Option<bool>,
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new(OutputFormat::Json)
    }
}

impl OutputFormatter {
    /// Create a new formatter with the specified output format.
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            include_metadata: true,
            pretty: None,
        }
    }

    /// Set whether to include metadata in output.
    pub fn with_metadata(mut self, include: bool) -> Self {
        self.include_metadata = include;
        self
    }

    /// Set pretty-printing preference.
    pub fn with_pretty(mut self, pretty: bool) -> Self {
        self.pretty = Some(pretty);
        self
    }

    /// Get the output format.
    pub fn format(&self) -> OutputFormat {
        self.format
    }

    /// Format a single value to string.
    pub fn format_value<T: Serialize>(&self, value: &T) -> Result<String> {
        match self.format {
            OutputFormat::Json => {
                let pretty = self.pretty.unwrap_or(true);
                if pretty {
                    Ok(serde_json::to_string_pretty(value)?)
                } else {
                    Ok(serde_json::to_string(value)?)
                }
            }
            OutputFormat::JsonLines => Ok(serde_json::to_string(value)?),
            OutputFormat::Csv => {
                // For CSV, serialize to JSON first then convert
                let json = serde_json::to_value(value)?;
                Ok(json_to_csv_line(&json))
            }
            OutputFormat::Human => {
                // For human-readable, use debug format with nice formatting
                let json = serde_json::to_value(value)?;
                Ok(json_to_human(&json, 0))
            }
        }
    }

    /// Format multiple values (for streaming output).
    pub fn format_iter<'a, T: Serialize + 'a>(
        &self,
        values: impl Iterator<Item = &'a T>,
    ) -> Result<String> {
        let mut output = String::new();
        match self.format {
            OutputFormat::Json => {
                // For JSON, collect into array
                let items: Vec<serde_json::Value> = values
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()?;
                let pretty = self.pretty.unwrap_or(true);
                if pretty {
                    output = serde_json::to_string_pretty(&items)?;
                } else {
                    output = serde_json::to_string(&items)?;
                }
            }
            OutputFormat::JsonLines => {
                // For JSONL, one line per item
                for value in values {
                    output.push_str(&serde_json::to_string(value)?);
                    output.push('\n');
                }
            }
            OutputFormat::Csv => {
                // For CSV, header + rows
                let items: Vec<serde_json::Value> = values
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(first) = items.first() {
                    // Header from first item
                    output.push_str(&json_to_csv_header(first));
                    output.push('\n');
                }
                for item in &items {
                    output.push_str(&json_to_csv_line(item));
                    output.push('\n');
                }
            }
            OutputFormat::Human => {
                for (i, value) in values.enumerate() {
                    let json = serde_json::to_value(value)?;
                    if i > 0 {
                        output.push_str("\n---\n\n");
                    }
                    output.push_str(&json_to_human(&json, 0));
                }
            }
        }
        Ok(output)
    }

    /// Write formatted output to a writer.
    pub fn write_value<T: Serialize, W: Write>(&self, value: &T, writer: &mut W) -> Result<()> {
        let output = self.format_value(value)?;
        writer.write_all(output.as_bytes())?;
        if self.format != OutputFormat::JsonLines {
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    /// Write a single item in streaming mode (for JSONL).
    pub fn write_streaming<T: Serialize, W: Write>(&self, value: &T, writer: &mut W) -> Result<()> {
        match self.format {
            OutputFormat::JsonLines => {
                serde_json::to_writer(&mut *writer, value)?;
                writer.write_all(b"\n")?;
            }
            _ => {
                // For non-streaming formats, just write the value
                self.write_value(value, writer)?;
            }
        }
        Ok(())
    }
}

/// Convert JSON value to CSV header (comma-separated field names).
fn json_to_csv_header(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => map
            .keys()
            .map(|k| escape_csv_field(k))
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    }
}

/// Convert JSON value to a single CSV line.
fn json_to_csv_line(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => map
            .values()
            .map(json_value_to_csv_field)
            .collect::<Vec<_>>()
            .join(","),
        _ => json_value_to_csv_field(value),
    }
}

/// Convert a single JSON value to a CSV field.
fn json_value_to_csv_field(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => escape_csv_field(s),
        serde_json::Value::Array(arr) => {
            // For arrays, join with semicolons
            let items: Vec<String> = arr.iter().map(json_value_to_csv_field).collect();
            escape_csv_field(&items.join(";"))
        }
        serde_json::Value::Object(_) => {
            // For nested objects, serialize to JSON
            escape_csv_field(&serde_json::to_string(value).unwrap_or_default())
        }
    }
}

/// Escape a CSV field (quote if necessary).
fn escape_csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Convert JSON value to human-readable format.
fn json_to_human(value: &serde_json::Value, indent: usize) -> String {
    let prefix = "  ".repeat(indent);
    match value {
        serde_json::Value::Null => format!("{}(null)", prefix),
        serde_json::Value::Bool(b) => format!("{}{}", prefix, b),
        serde_json::Value::Number(n) => format!("{}{}", prefix, n),
        serde_json::Value::String(s) => format!("{}{}", prefix, s),
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                format!("{}(empty list)", prefix)
            } else {
                let items: Vec<String> = arr.iter().map(|v| json_to_human(v, indent + 1)).collect();
                format!("{}\n{}", prefix, items.join("\n"))
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                format!("{}(empty)", prefix)
            } else {
                let items: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        if v.is_object() || v.is_array() {
                            format!("{}{}:\n{}", prefix, k, json_to_human(v, indent + 1))
                        } else {
                            format!("{}{}: {}", prefix, k, json_value_simple(v))
                        }
                    })
                    .collect();
                items.join("\n")
            }
        }
    }
}

/// Simple single-line representation of a JSON value.
fn json_value_simple(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "(null)".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(OutputFormat::from_str("json"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::from_str("JSON"), Some(OutputFormat::Json));
        assert_eq!(
            OutputFormat::from_str("jsonl"),
            Some(OutputFormat::JsonLines)
        );
        assert_eq!(OutputFormat::from_str("csv"), Some(OutputFormat::Csv));
        assert_eq!(OutputFormat::from_str("human"), Some(OutputFormat::Human));
        assert_eq!(OutputFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_format_json() {
        let formatter = OutputFormatter::new(OutputFormat::Json);
        let data = serde_json::json!({"name": "test", "value": 42});
        let output = formatter.format_value(&data).unwrap();
        assert!(output.contains("\"name\": \"test\""));
        assert!(output.contains("\"value\": 42"));
    }

    #[test]
    fn test_format_jsonl() {
        let formatter = OutputFormatter::new(OutputFormat::JsonLines);
        let data = serde_json::json!({"name": "test"});
        let output = formatter.format_value(&data).unwrap();
        assert_eq!(output, "{\"name\":\"test\"}");
    }

    #[test]
    fn test_format_csv() {
        let formatter = OutputFormatter::new(OutputFormat::Csv);
        let data = serde_json::json!({"name": "test", "value": 42});
        let output = formatter.format_value(&data).unwrap();
        // CSV should contain the values
        assert!(output.contains("test") || output.contains("42"));
    }

    #[test]
    fn test_escape_csv_field() {
        assert_eq!(escape_csv_field("simple"), "simple");
        assert_eq!(escape_csv_field("with,comma"), "\"with,comma\"");
        assert_eq!(escape_csv_field("with\"quote"), "\"with\"\"quote\"");
    }
}
