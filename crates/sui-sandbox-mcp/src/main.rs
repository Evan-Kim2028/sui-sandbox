use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData as McpError, ServiceExt,
};
use serde_json::Value;

use std::sync::Arc;
use sui_sandbox_mcp::ToolDispatcher;

#[derive(Clone)]
struct SandboxMcpServer {
    dispatcher: Arc<ToolDispatcher>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SandboxMcpServer {
    fn new(dispatcher: ToolDispatcher) -> Self {
        Self {
            dispatcher: Arc::new(dispatcher),
            tool_router: Self::tool_router(),
        }
    }

    async fn dispatch_tool(
        &self,
        name: &str,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        let response = self.dispatcher.dispatch(name, params.0).await;
        let content_text = if response.success {
            "ok".to_string()
        } else {
            response
                .error
                .clone()
                .unwrap_or_else(|| "error".to_string())
        };
        Ok(CallToolResult {
            content: vec![Content::text(content_text)],
            structured_content: Some(serde_json::to_value(&response).unwrap_or(Value::Null)),
            is_error: Some(!response.success),
            meta: None,
        })
    }

    #[tool(name = "call_function", description = "Call a single Move function")]
    async fn call_function(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("call_function", params).await
    }

    #[tool(
        name = "execute_ptb",
        description = "Execute a Programmable Transaction Block"
    )]
    async fn execute_ptb(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("execute_ptb", params).await
    }

    #[tool(
        name = "replay_transaction",
        description = "Replay a historical mainnet transaction"
    )]
    async fn replay_transaction(
        &self,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("replay_transaction", params).await
    }

    #[tool(
        name = "create_move_project",
        description = "Create a new Move project"
    )]
    async fn create_move_project(
        &self,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("create_move_project", params).await
    }

    #[tool(name = "read_move_file", description = "Read a Move source file")]
    async fn read_move_file(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("read_move_file", params).await
    }

    #[tool(name = "edit_move_file", description = "Edit a Move source file")]
    async fn edit_move_file(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("edit_move_file", params).await
    }

    #[tool(name = "build_project", description = "Compile a Move project")]
    async fn build_project(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("build_project", params).await
    }

    #[tool(name = "test_project", description = "Run Move unit tests")]
    async fn test_project(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("test_project", params).await
    }

    #[tool(
        name = "deploy_project",
        description = "Compile and deploy a Move project"
    )]
    async fn deploy_project(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("deploy_project", params).await
    }

    #[tool(name = "list_projects", description = "List all Move projects")]
    async fn list_projects(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("list_projects", params).await
    }

    #[tool(
        name = "list_packages",
        description = "List deployed packages in the sandbox"
    )]
    async fn list_packages(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("list_packages", params).await
    }

    #[tool(
        name = "set_active_package",
        description = "Pin a project to a package id"
    )]
    async fn set_active_package(
        &self,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("set_active_package", params).await
    }

    #[tool(
        name = "upgrade_project",
        description = "Upgrade a project package locally"
    )]
    async fn upgrade_project(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("upgrade_project", params).await
    }

    #[tool(name = "read_object", description = "Read an object from the sandbox")]
    async fn read_object(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("read_object", params).await
    }

    #[tool(
        name = "create_asset",
        description = "Create test coins or synthetic objects"
    )]
    async fn create_asset(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("create_asset", params).await
    }

    #[tool(
        name = "load_from_mainnet",
        description = "Fetch package or object from mainnet"
    )]
    async fn load_from_mainnet(
        &self,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("load_from_mainnet", params).await
    }

    #[tool(
        name = "load_package_bytes",
        description = "Load local package bytecode into the sandbox"
    )]
    async fn load_package_bytes(
        &self,
        params: Parameters<Value>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("load_package_bytes", params).await
    }

    #[tool(
        name = "get_interface",
        description = "Get module interface information"
    )]
    async fn get_interface(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("get_interface", params).await
    }

    #[tool(name = "search", description = "Search functions or types")]
    async fn search(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("search", params).await
    }

    #[tool(name = "get_state", description = "Get sandbox state summary")]
    async fn get_state(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("get_state", params).await
    }

    #[tool(
        name = "configure",
        description = "Configure sandbox environment settings"
    )]
    async fn configure(&self, params: Parameters<Value>) -> Result<CallToolResult, McpError> {
        self.dispatch_tool("configure", params).await
    }
}

#[tool_handler]
impl rmcp::ServerHandler for SandboxMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "sui-sandbox MCP server. Use tools like create_move_project, execute_ptb, and replay_transaction."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dispatcher = ToolDispatcher::new()?;
    let server = SandboxMcpServer::new(dispatcher);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
