use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::client::{McpError, McpNotification, McpTransport};
use crate::db::repos::Database;
use crate::runtime::wasm_extension::WasmExtensionRunner;

pub struct WasmStdioTransport {
    db: Arc<Database>,
    extension_id: String,
    #[allow(dead_code)]
    wasm_hash: String,
    manifest: Value,
    notification_tx: mpsc::UnboundedSender<McpNotification>,
    data_dir: PathBuf,
}

impl WasmStdioTransport {
    pub fn new(
        db: Arc<Database>,
        extension_id: String,
        wasm_hash: String,
        manifest: Value,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            extension_id,
            wasm_hash,
            manifest,
            notification_tx,
            data_dir,
        }
    }

    /// Emit an unsolicited push notification from this WASM extension
    pub fn push_notification(&self, method: &str, params: Option<Value>) {
        let _ = self.notification_tx.send(McpNotification {
            method: method.to_string(),
            params,
        });
    }
}

#[async_trait]
impl McpTransport for WasmStdioTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        match method {
            "initialize" => {
                let name = self
                    .manifest
                    .get("mcp")
                    .and_then(|m| m.get("server_name"))
                    .and_then(|s| s.as_str())
                    .or_else(|| self.manifest.get("name").and_then(|s| s.as_str()))
                    .unwrap_or("WasmMcpServer");
                let version = self
                    .manifest
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("1.0.0");

                Ok(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": { "listChanged": true },
                        "resources": { "subscribe": true, "listChanged": true }
                    },
                    "serverInfo": {
                        "name": name,
                        "version": version
                    }
                }))
            }

            "tools/list" => {
                let tools = self
                    .manifest
                    .get("mcp")
                    .and_then(|m| m.get("tools"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                Ok(serde_json::json!({ "tools": tools }))
            }

            "tools/call" => {
                let params_val = params.unwrap_or(serde_json::Value::Null);
                let ext = self
                    .db
                    .get_extension(&self.extension_id)
                    .await
                    .map_err(|e| McpError::ServerError {
                        code: -32603,
                        message: e.to_string(),
                    })?
                    .ok_or_else(|| McpError::ToolNotFound("Extension not found".to_string()))?;

                let runner = WasmExtensionRunner::new(
                    &ext.wasm_binary,
                    ext.id.clone(),
                    &self.data_dir,
                    Arc::clone(&self.db),
                )
                .map_err(|e| McpError::ServerError {
                    code: -32603,
                    message: e.to_string(),
                })?;

                let res_str = runner
                    .handle_mcp_request("tools/call", &params_val.to_string())
                    .map_err(|e| McpError::ServerError {
                        code: -32603,
                        message: e.to_string(),
                    })?;

                // Update storage size after tool execution
                let new_storage = runner.calculate_storage_usage().await;
                let _ = self.db.update_extension_storage(&self.extension_id, new_storage).await;

                let res_val: Value = serde_json::from_str(&res_str).unwrap_or_else(|_| {
                    serde_json::json!({
                        "content": [{ "type": "text", "text": res_str }]
                    })
                });

                Ok(res_val)
            }

            "resources/list" => {
                let resources = self
                    .manifest
                    .get("mcp")
                    .and_then(|m| m.get("resources"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                Ok(serde_json::json!({ "resources": resources }))
            }

            "resources/read" => {
                let params_val = params.unwrap_or(serde_json::Value::Null);
                let ext = self
                    .db
                    .get_extension(&self.extension_id)
                    .await
                    .map_err(|e| McpError::ServerError {
                        code: -32603,
                        message: e.to_string(),
                    })?
                    .ok_or_else(|| McpError::ToolNotFound("Extension not found".to_string()))?;

                let runner = WasmExtensionRunner::new(
                    &ext.wasm_binary,
                    ext.id.clone(),
                    &self.data_dir,
                    Arc::clone(&self.db),
                )
                .map_err(|e| McpError::ServerError {
                    code: -32603,
                    message: e.to_string(),
                })?;

                let res_str = runner
                    .handle_mcp_request("resources/read", &params_val.to_string())
                    .map_err(|e| McpError::ServerError {
                        code: -32603,
                        message: e.to_string(),
                    })?;

                let res_val: Value = serde_json::from_str(&res_str).unwrap_or_else(|_| {
                    let uri = params_val.get("uri").and_then(|u| u.as_str()).unwrap_or("ui://app");
                    serde_json::json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/html;profile=mcp-app",
                            "text": res_str
                        }]
                    })
                });

                Ok(res_val)
            }

            "resources/subscribe" => {
                if let Some(uri) = params.as_ref().and_then(|p| p.get("uri")).and_then(|u| u.as_str()) {
                    info!(
                        "Agent subscribed to WASM MCP resource '{}' on extension '{}'",
                        uri, self.extension_id
                    );
                }
                Ok(serde_json::json!({}))
            }

            "resources/unsubscribe" => {
                Ok(serde_json::json!({}))
            }

            _ => {
                warn!("Unhandled WASM MCP method '{}'", method);
                Ok(serde_json::json!({ "result": "unhandled" }))
            }
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}
