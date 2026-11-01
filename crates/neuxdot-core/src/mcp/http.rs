use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

use super::client::{JsonRpcRequest, JsonRpcResponse, McpError, McpNotification, McpTransport};

pub struct HttpTransport {
    url: String,
    client: Client,
    next_id: AtomicI64,
}

impl HttpTransport {
    pub fn new(
        url: &str,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Arc<Self> {
        let client = Client::builder().build().unwrap_or_default();
        let sse_url = url.to_string();

        // Background SSE/Stream listener for server pushed information
        let sse_client = client.clone();
        tokio::spawn(async move {
            let stream_url = if sse_url.ends_with("/sse") {
                sse_url.clone()
            } else if sse_url.ends_with('/') {
                format!("{}sse", sse_url)
            } else {
                format!("{}/sse", sse_url)
            };

            if let Ok(res) = sse_client.get(&stream_url).send().await {
                if res.status().is_success() {
                    let mut stream = res.bytes_stream();
                    while let Some(item) = stream.next().await {
                        if let Ok(bytes) = item {
                            let text = String::from_utf8_lossy(&bytes);
                            for line in text.lines() {
                                if let Some(stripped) = line.strip_prefix("data: ") {
                                    if let Ok(val) = serde_json::from_str::<Value>(stripped) {
                                        if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                                            info!("MCP HTTP/SSE Server pushed: {}", method);
                                            let _ = notification_tx.send(McpNotification {
                                                method: method.to_string(),
                                                params: val.get("params").cloned(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        Arc::new(Self {
            url: url.to_string(),
            client,
            next_id: AtomicI64::new(1),
        })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Value::from(id),
            method: method.to_string(),
            params,
        };

        let res = self
            .client
            .post(&self.url)
            .json(&req)
            .send()
            .await
            .map_err(McpError::Http)?;

        let resp: JsonRpcResponse = res.json().await.map_err(McpError::Http)?;

        if let Some(err) = resp.error {
            return Err(McpError::ServerError {
                code: err.code,
                message: err.message,
            });
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}
