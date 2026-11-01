use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

use super::client::{JsonRpcRequest, JsonRpcResponse, McpError, McpNotification, McpTransport};

pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, McpError>>>>>,
    next_id: AtomicI64,
    child: Mutex<Child>,
}

impl StdioTransport {
    pub async fn spawn(
        command_line: &str,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Result<Arc<Self>, McpError> {
        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return Err(McpError::Protocol("Empty command string".to_string()));
        }

        let cmd = parts[0];
        let args = &parts[1..];

        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(McpError::Io)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("Failed to open child stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("Failed to open child stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Protocol("Failed to open child stderr".to_string()))?;

        let pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, McpError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pending_clone = Arc::clone(&pending_requests);

        // Background reader task for stdout
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                debug!("MCP stdio incoming: {}", trimmed);

                // Attempt to parse JSON-RPC
                if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
                    // Check if it's a notification (no 'id', but has 'method')
                    if value.get("id").is_none() && value.get("method").is_some() {
                        let method = value["method"].as_str().unwrap_or("").to_string();
                        let params = value.get("params").cloned();
                        info!("MCP Server pushed notification: method={}", method);
                        let _ = notification_tx.send(McpNotification { method, params });
                        continue;
                    }

                    // Otherwise it's a response
                    if let Ok(resp) = serde_json::from_value::<JsonRpcResponse>(value) {
                        if let Some(id_val) = resp.id {
                            let id_num = id_val.as_i64().unwrap_or(-1);
                            let mut map = pending_clone.lock().await;
                            if let Some(sender) = map.remove(&id_num) {
                                if let Some(err) = resp.error {
                                    let _ = sender.send(Err(McpError::ServerError {
                                        code: err.code,
                                        message: err.message,
                                    }));
                                } else {
                                    let result = resp.result.unwrap_or(Value::Null);
                                    let _ = sender.send(Ok(result));
                                }
                            }
                        }
                    }
                }
            }
        });

        // Background reader for stderr (for logs)
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                warn!("MCP stdio stderr: {}", line);
            }
        });

        Ok(Arc::new(Self {
            stdin: Mutex::new(stdin),
            pending_requests,
            next_id: AtomicI64::new(1),
            child: Mutex::new(child),
        }))
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Value::from(id),
            method: method.to_string(),
            params,
        };

        let json_line = serde_json::to_string(&req)? + "\n";

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending_requests.lock().await;
            map.insert(id, tx);
        }

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(json_line.as_bytes()).await?;
            stdin.flush().await?;
        }

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(McpError::ConnectionClosed),
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}
