use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::client::{JsonRpcRequest, JsonRpcResponse, McpError, McpNotification, McpTransport};

pub fn parse_custom_headers(raw: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return headers;
    }

    // Try parsing as JSON object first: e.g. {"Authorization": "Bearer ...", "X-API-Key": "..."}
    if trimmed.starts_with('{') {
        if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(trimmed) {
            for (k, v) in map {
                let v_str = match v {
                    Value::String(s) => s,
                    _ => v.to_string(),
                };
                if let (Ok(name), Ok(val)) = (
                    HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(&v_str),
                ) {
                    headers.insert(name, val);
                } else {
                    warn!("Invalid HTTP header name or value: {} = {}", k, v_str);
                }
            }
            return headers;
        }
    }

    // Otherwise, parse line by line: "Key: Value" or "Key=Value"
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sep = if line.contains(':') {
            ':'
        } else if line.contains('=') {
            '='
        } else {
            continue;
        };

        if let Some((k, v)) = line.split_once(sep) {
            let key = k.trim();
            let val = v.trim();
            if let (Ok(name), Ok(header_val)) = (
                HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(val),
            ) {
                headers.insert(name, header_val);
            } else {
                warn!("Invalid HTTP header name or value: {} = {}", key, val);
            }
        }
    }

    headers
}

pub struct HttpTransport {
    url: String,
    client: Client,
    next_id: AtomicI64,
}

impl HttpTransport {
    pub fn new(
        url: &str,
        headers_raw: Option<&str>,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Arc<Self> {
        let mut builder = Client::builder();
        if let Some(raw) = headers_raw {
            let parsed_headers = parse_custom_headers(raw);
            if !parsed_headers.is_empty() {
                builder = builder.default_headers(parsed_headers);
            }
        }
        let client = builder.build().unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_headers() {
        let raw = r#"{"Authorization": "Bearer test-token-123", "X-Custom": "val"}"#;
        let headers = parse_custom_headers(raw);
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test-token-123"
        );
        assert_eq!(headers.get("x-custom").unwrap().to_str().unwrap(), "val");
    }

    #[test]
    fn test_parse_lines_headers() {
        let raw = "Authorization: Bearer my-key\nX-Api-Key: 12345\n";
        let headers = parse_custom_headers(raw);
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer my-key"
        );
        assert_eq!(headers.get("x-api-key").unwrap().to_str().unwrap(), "12345");
    }
}
