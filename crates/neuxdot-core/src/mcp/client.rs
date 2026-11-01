use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

#[derive(Error, Debug)]
pub enum McpError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Server error ({code}): {message}")]
    ServerError { code: i64, message: String },
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    #[error("Connection closed")]
    ConnectionClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObj>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorObj {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError>;
    async fn close(&self) -> Result<(), McpError>;
}

/// A client session for an MCP server, receiving unsolicited push notifications
pub struct McpClientSession {
    pub server_id: String,
    pub server_name: String,
    pub transport: Arc<dyn McpTransport>,
    pub notification_rx: Mutex<mpsc::UnboundedReceiver<McpNotification>>,
}

impl McpClientSession {
    pub async fn initialize(&self) -> Result<Value, McpError> {
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "NeuxDOT",
                "version": "0.1.0"
            }
        });

        self.transport.send_request("initialize", Some(init_params)).await
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let res = self.transport.send_request("tools/list", None).await?;
        if let Some(tools_val) = res.get("tools") {
            let tools: Vec<McpTool> = serde_json::from_value(tools_val.clone())?;
            Ok(tools)
        } else {
            Ok(vec![])
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });
        self.transport.send_request("tools/call", Some(params)).await
    }
}
