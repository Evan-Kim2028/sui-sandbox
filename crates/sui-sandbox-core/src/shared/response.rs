//! Unified tool response types.
//!
//! This module provides a unified response type that can be used by both
//! CLI and MCP to ensure consistent output formatting.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified response type for tool execution.
///
/// This type is used by both CLI and MCP to return results from tool execution.
/// It provides a consistent structure for success/error states, warnings, and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    /// Whether the operation succeeded.
    pub success: bool,

    /// The result value (JSON for flexibility).
    pub result: Value,

    /// Error message if the operation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Additional error details (stack trace, context, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_details: Option<Value>,

    /// Non-fatal warnings generated during execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    /// Whether the result came from cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit: Option<bool>,

    /// Execution duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolResponse {
    /// Create a successful response with a result value.
    pub fn ok(result: Value) -> Self {
        Self {
            success: true,
            result,
            error: None,
            error_details: None,
            warnings: Vec::new(),
            cache_hit: None,
            duration_ms: None,
        }
    }

    /// Create a successful response with an empty result.
    pub fn ok_empty() -> Self {
        Self::ok(Value::Null)
    }

    /// Create an error response with a message.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            result: Value::Null,
            error: Some(message.into()),
            error_details: None,
            warnings: Vec::new(),
            cache_hit: None,
            duration_ms: None,
        }
    }

    /// Create an error response from an anyhow::Error.
    pub fn from_error(err: &anyhow::Error) -> Self {
        let mut response = Self::error(err.to_string());

        // Capture the error chain as details
        let chain: Vec<String> = err.chain().skip(1).map(|e| e.to_string()).collect();
        if !chain.is_empty() {
            response.error_details = Some(serde_json::json!({
                "cause_chain": chain
            }));
        }

        response
    }

    /// Add error details to the response.
    pub fn with_details(mut self, details: Value) -> Self {
        self.error_details = Some(details);
        self
    }

    /// Add a warning to the response.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Add multiple warnings to the response.
    pub fn with_warnings(mut self, warnings: impl IntoIterator<Item = String>) -> Self {
        self.warnings.extend(warnings);
        self
    }

    /// Set the cache hit flag.
    pub fn with_cache_hit(mut self, hit: bool) -> Self {
        self.cache_hit = Some(hit);
        self
    }

    /// Set the execution duration.
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    /// Convert the response to a JSON Value.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Check if this is an error response.
    pub fn is_error(&self) -> bool {
        !self.success
    }

    /// Get the error message if this is an error response.
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Default for ToolResponse {
    fn default() -> Self {
        Self::ok_empty()
    }
}

impl From<anyhow::Error> for ToolResponse {
    fn from(err: anyhow::Error) -> Self {
        Self::from_error(&err)
    }
}

impl<T: Serialize> From<Result<T, anyhow::Error>> for ToolResponse {
    fn from(result: Result<T, anyhow::Error>) -> Self {
        match result {
            Ok(value) => {
                let json = serde_json::to_value(value).unwrap_or(Value::Null);
                Self::ok(json)
            }
            Err(err) => Self::from_error(&err),
        }
    }
}

/// Extract and deserialize a tool input from a JSON Value.
///
/// This helper eliminates the repeated pattern of:
/// ```ignore
/// let parsed: SomeInput = match serde_json::from_value(input) {
///     Ok(v) => v,
///     Err(e) => return ToolResponse::error(format!("Invalid input: {}", e)),
/// };
/// ```
///
/// # Example
/// ```ignore
/// use sui_sandbox_core::shared::response::extract_input;
///
/// let result = extract_input::<MyInputType>(value)?;
/// // If extraction fails, returns Err(ToolResponse::error(...))
/// ```
pub fn extract_input<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ToolResponse> {
    serde_json::from_value(value).map_err(|e| ToolResponse::error(format!("Invalid input: {}", e)))
}

/// Extract and deserialize a tool input, with a custom error prefix.
pub fn extract_input_with_context<T: serde::de::DeserializeOwned>(
    value: Value,
    context: &str,
) -> Result<T, ToolResponse> {
    serde_json::from_value(value)
        .map_err(|e| ToolResponse::error(format!("{}: {}", context, e)))
}

/// Metadata that can be attached to tool invocations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolMeta {
    /// Reason for the tool invocation (for logging/debugging).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Unique request ID for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Tags for categorization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

impl ToolMeta {
    /// Create new metadata with a reason.
    pub fn with_reason(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Default::default()
        }
    }

    /// Create new metadata with a request ID.
    pub fn with_request_id(request_id: impl Into<String>) -> Self {
        Self {
            request_id: Some(request_id.into()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ok_response() {
        let response = ToolResponse::ok(serde_json::json!({"value": 42}));
        assert!(response.success);
        assert_eq!(response.result["value"], 42);
        assert!(response.error.is_none());
    }

    #[test]
    fn test_error_response() {
        let response = ToolResponse::error("Something went wrong");
        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("Something went wrong"));
    }

    #[test]
    fn test_with_warning() {
        let response = ToolResponse::ok_empty()
            .with_warning("Warning 1")
            .with_warning("Warning 2");
        assert_eq!(response.warnings.len(), 2);
    }

    #[test]
    fn test_from_anyhow_error() {
        let err = anyhow::anyhow!("Test error");
        let response = ToolResponse::from_error(&err);
        assert!(!response.success);
        assert!(response.error.as_deref().unwrap().contains("Test error"));
    }

    #[test]
    fn test_json_serialization() {
        let response = ToolResponse::ok(serde_json::json!({"key": "value"}));
        let json = response.to_json();
        assert_eq!(json["success"], true);
        assert_eq!(json["result"]["key"], "value");
    }
}
