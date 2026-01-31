//! Shared tool registry and trait definitions.
//!
//! This module provides a unified tool system that can be used by both CLI and MCP.
//! Tools are defined as trait implementations, allowing them to be registered once
//! and invoked from either interface.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    ToolRegistry                              │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
//! │  │ CallFunction │  │ ExecutePtb   │  │ ReadObject   │  ...  │
//! │  └──────────────┘  └──────────────┘  └──────────────┘       │
//! │         │                 │                 │                │
//! │         └─────────────────┴─────────────────┘                │
//! │                           │                                  │
//! │                    ┌──────▼──────┐                           │
//! │                    │  Tool Trait │                           │
//! │                    └─────────────┘                           │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!              ┌─────────────┴─────────────┐
//!              │                           │
//!       ┌──────▼──────┐            ┌───────▼──────┐
//!       │    CLI      │            │     MCP      │
//!       │  (clap)     │            │   (rmcp)     │
//!       └─────────────┘            └──────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use sui_sandbox_core::shared::tools::{Tool, ToolContext, ToolInput, ToolRegistry};
//!
//! // Create a registry and get the default tools
//! let registry = ToolRegistry::with_defaults();
//!
//! // Execute a tool
//! let input = ToolInput::from_json(json!({
//!     "package": "0x2",
//!     "module": "coin",
//!     "function": "balance"
//! }));
//! let result = registry.execute("call_function", input, &context).await;
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::simulation::SimulationEnvironment;

use super::response::ToolResponse;

/// Input to a tool, either from CLI args or JSON.
#[derive(Debug, Clone)]
pub enum ToolInput {
    /// JSON input (from MCP or CLI --json flag)
    Json(Value),
    /// Structured arguments (from CLI positional args)
    Args {
        positional: Vec<String>,
        named: HashMap<String, String>,
    },
}

impl ToolInput {
    /// Create input from a JSON value.
    pub fn from_json(value: Value) -> Self {
        ToolInput::Json(value)
    }

    /// Create input from CLI-style arguments.
    pub fn from_args(positional: Vec<String>, named: HashMap<String, String>) -> Self {
        ToolInput::Args { positional, named }
    }

    /// Get the input as JSON, converting if necessary.
    pub fn as_json(&self) -> Value {
        match self {
            ToolInput::Json(v) => v.clone(),
            ToolInput::Args { positional, named } => {
                let mut map = serde_json::Map::new();
                map.insert("_positional".to_string(), Value::Array(
                    positional.iter().map(|s| Value::String(s.clone())).collect()
                ));
                for (k, v) in named {
                    map.insert(k.clone(), Value::String(v.clone()));
                }
                Value::Object(map)
            }
        }
    }

    /// Get a string value from the input.
    pub fn get_string(&self, key: &str) -> Option<String> {
        match self {
            ToolInput::Json(v) => v.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()),
            ToolInput::Args { named, .. } => named.get(key).cloned(),
        }
    }

    /// Get a u64 value from the input.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        match self {
            ToolInput::Json(v) => v.get(key).and_then(|v| v.as_u64()),
            ToolInput::Args { named, .. } => named.get(key).and_then(|s| s.parse().ok()),
        }
    }

    /// Get a boolean value from the input.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self {
            ToolInput::Json(v) => v.get(key).and_then(|v| v.as_bool()),
            ToolInput::Args { named, .. } => named.get(key).and_then(|s| s.parse().ok()),
        }
    }

    /// Get an array value from the input.
    pub fn get_array(&self, key: &str) -> Option<Vec<Value>> {
        match self {
            ToolInput::Json(v) => v.get(key).and_then(|v| v.as_array()).cloned(),
            ToolInput::Args { .. } => None,
        }
    }

    /// Get a required string value, returning an error if missing.
    pub fn require_string(&self, key: &str) -> Result<String, String> {
        self.get_string(key)
            .ok_or_else(|| format!("Missing required parameter: {}", key))
    }
}

/// Context provided to tools during execution.
///
/// This contains shared state and services that tools need to do their work.
pub struct ToolContext {
    /// The simulation environment.
    pub env: Arc<Mutex<SimulationEnvironment>>,

    /// Network configuration.
    pub network: String,

    /// Whether to use caching.
    pub cache_enabled: bool,

    /// Additional context data.
    pub extra: HashMap<String, Value>,
}

impl ToolContext {
    /// Create a new context with the given environment.
    pub fn new(env: Arc<Mutex<SimulationEnvironment>>) -> Self {
        Self {
            env,
            network: "mainnet".to_string(),
            cache_enabled: true,
            extra: HashMap::new(),
        }
    }

    /// Set the network.
    pub fn with_network(mut self, network: impl Into<String>) -> Self {
        self.network = network.into();
        self
    }

    /// Set whether caching is enabled.
    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self
    }

    /// Add extra context data.
    pub fn with_extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// Trait for tool implementations.
///
/// Each tool implements this trait to define its behavior. Tools can be
/// registered with a ToolRegistry and invoked from CLI or MCP.
pub trait Tool: Send + Sync {
    /// The tool's unique name (used for dispatch).
    fn name(&self) -> &'static str;

    /// A short description of what the tool does.
    fn description(&self) -> &'static str;

    /// Execute the tool with the given input and context.
    fn execute<'a>(
        &'a self,
        input: ToolInput,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResponse> + Send + 'a>>;

    /// Get the JSON schema for this tool's input (for MCP).
    fn input_schema(&self) -> Option<Value> {
        None
    }
}

/// Descriptor for a registered tool.
#[derive(Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// The tool's name.
    pub name: String,
    /// The tool's description.
    pub description: String,
    /// The input schema (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

/// Registry of available tools.
///
/// The registry holds all registered tools and provides dispatch functionality.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::new(tool));
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// List all registered tools.
    pub fn list(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|t| ToolDescriptor {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    /// Execute a tool by name.
    pub async fn execute(
        &self,
        name: &str,
        input: ToolInput,
        context: &ToolContext,
    ) -> ToolResponse {
        match self.tools.get(name) {
            Some(tool) => tool.execute(input, context).await,
            None => ToolResponse::error(format!("Unknown tool: {}", name)),
        }
    }

    /// Check if a tool is registered.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Built-in Tool Implementations
// ============================================================================

/// Tool for getting the current simulation state.
pub struct GetStateTool;

impl Tool for GetStateTool {
    fn name(&self) -> &'static str {
        "get_state"
    }

    fn description(&self) -> &'static str {
        "Get the current simulation state summary"
    }

    fn execute<'a>(
        &'a self,
        _input: ToolInput,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResponse> + Send + 'a>> {
        Box::pin(async move {
            let env = context.env.lock();
            let summary = env.get_state_summary();
            ToolResponse::ok(serde_json::json!({
                "object_count": summary.object_count,
                "loaded_packages": summary.loaded_packages,
                "loaded_modules": summary.loaded_modules,
                "sender": summary.sender,
                "timestamp_ms": summary.timestamp_ms,
                "network": context.network,
            }))
        })
    }

    fn input_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }))
    }
}

/// Tool for listing available packages.
pub struct ListPackagesTool;

impl Tool for ListPackagesTool {
    fn name(&self) -> &'static str {
        "list_packages"
    }

    fn description(&self) -> &'static str {
        "List all packages in the simulation environment"
    }

    fn execute<'a>(
        &'a self,
        _input: ToolInput,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResponse> + Send + 'a>> {
        Box::pin(async move {
            let env = context.env.lock();
            let packages: Vec<String> = env
                .list_packages()
                .iter()
                .map(|addr| addr.to_hex_literal())
                .collect();
            ToolResponse::ok(serde_json::json!({
                "packages": packages,
                "count": packages.len()
            }))
        })
    }

    fn input_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_input_from_json() {
        let input = ToolInput::from_json(serde_json::json!({
            "key": "value",
            "num": 42
        }));

        assert_eq!(input.get_string("key"), Some("value".to_string()));
        assert_eq!(input.get_u64("num"), Some(42));
    }

    #[test]
    fn test_tool_input_from_args() {
        let mut named = HashMap::new();
        named.insert("key".to_string(), "value".to_string());
        named.insert("num".to_string(), "42".to_string());

        let input = ToolInput::from_args(vec!["pos1".to_string()], named);

        assert_eq!(input.get_string("key"), Some("value".to_string()));
        assert_eq!(input.get_u64("num"), Some(42));
    }

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        registry.register(GetStateTool);
        registry.register(ListPackagesTool);

        assert!(registry.has_tool("get_state"));
        assert!(registry.has_tool("list_packages"));
        assert!(!registry.has_tool("unknown"));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_tool_descriptors() {
        let mut registry = ToolRegistry::new();
        registry.register(GetStateTool);

        let tools = registry.list();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_state");
    }
}
