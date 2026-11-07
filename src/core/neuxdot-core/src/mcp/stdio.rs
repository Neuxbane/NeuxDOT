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

pub fn parse_cli_args(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('[') {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(trimmed) {
            return list;
        }
    }
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for c in trimmed.chars() {
        match c {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = c;
            }
            c if in_quotes && c == quote_char => {
                in_quotes = false;
            }
            ' ' | '\t' | '\r' | '\n' if !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub fn parse_env_vars(raw: &str) -> HashMap<String, String> {
    let mut envs = HashMap::new();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return envs;
    }

    // Try parsing as JSON object: {"KEY": "VAL"}
    if trimmed.starts_with('{') {
        if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(trimmed) {
            for (k, v) in map {
                let v_str = match v {
                    Value::String(s) => s,
                    _ => v.to_string(),
                };
                envs.insert(k, v_str);
            }
            return envs;
        }
    }

    // Otherwise, parse line by line: "KEY=VALUE" or "KEY: VALUE"
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sep = if line.contains('=') {
            '='
        } else if line.contains(':') {
            ':'
        } else {
            continue;
        };

        if let Some((k, v)) = line.split_once(sep) {
            envs.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    envs
}

pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, McpError>>>>>,
    next_id: AtomicI64,
    child: Mutex<Child>,
}

impl StdioTransport {
    pub async fn spawn(
        command_line: &str,
        env_raw: Option<&str>,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Result<Arc<Self>, McpError> {
        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return Err(McpError::Protocol("Empty command string".to_string()));
        }

        let cmd = parts[0];
        let args = &parts[1..];

        let mut command = Command::new(cmd);
        command.args(args);

        if let Some(raw) = env_raw {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') || trimmed.starts_with('-') {
                command.args(parse_cli_args(trimmed));
            } else if trimmed.starts_with('{') {
                let envs = parse_env_vars(trimmed);
                for (k, v) in envs {
                    command.env(k, v);
                }
            } else if trimmed.lines().any(|l| l.contains('=') || l.contains(':')) {
                let envs = parse_env_vars(trimmed);
                for (k, v) in envs {
                    command.env(k, v);
                }
            } else {
                command.args(parse_cli_args(trimmed));
            }
        }

        let mut child = command
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
                let trimmed = line.trim();
                let lower = trimmed.to_ascii_lowercase();
                if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") {
                    warn!("MCP stdio stderr: {}", trimmed);
                } else {
                    debug!("MCP stdio stderr: {}", trimmed);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_envs() {
        let raw = r#"{"GITHUB_TOKEN": "ghp_12345", "DEBUG": "1"}"#;
        let envs = parse_env_vars(raw);
        assert_eq!(envs.get("GITHUB_TOKEN").unwrap(), "ghp_12345");
        assert_eq!(envs.get("DEBUG").unwrap(), "1");
    }

    #[test]
    fn test_parse_line_envs() {
        let raw = "API_KEY=my_secret_key\nPORT=8080\n";
        let envs = parse_env_vars(raw);
        assert_eq!(envs.get("API_KEY").unwrap(), "my_secret_key");
        assert_eq!(envs.get("PORT").unwrap(), "8080");
    }

    #[test]
    fn test_parse_cli_args() {
        let raw = "--token my-token --port 8080 --name 'My Service'";
        let args = parse_cli_args(raw);
        assert_eq!(args, vec!["--token", "my-token", "--port", "8080", "--name", "My Service"]);

        let json_raw = r#"["--token", "glpat-123", "--read-only"]"#;
        let args2 = parse_cli_args(json_raw);
        assert_eq!(args2, vec!["--token", "glpat-123", "--read-only"]);
    }
}
