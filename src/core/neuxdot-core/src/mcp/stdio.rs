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
    /// Owns the sandbox allow-list proxy (and its unix socket) for the lifetime
    /// of the child. Dropping it here would abort the proxy task while the
    /// sandboxed plugin is still running, so every outbound request would fail
    /// with "error sending request". `None` for elevated / no-proxy launches.
    _proxy_guard: Option<Arc<crate::mcp::proxy::AllowListProxy>>,
}

/// True when the isolation binary is on PATH.
///
/// Checked once and cached: `bwrap` presence cannot change during a process
/// lifetime in any way that matters, and spawning a plugin is on the hot path.
fn sandbox_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let found = std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|dir| {
                    let candidate = dir.join("bwrap");
                    candidate.is_file()
                })
            })
            .unwrap_or(false);
        if !found {
            warn!("bubblewrap ('bwrap') not found on PATH; sandboxed plugins cannot start");
        }
        found
    })
}

impl StdioTransport {
    /// Build the process command for `launch`.
    ///
    /// Returns the command plus the guard that must outlive the child: when a
    /// network allow-list is configured the guard owns the proxy socket and its
    /// task, and dropping it while the child runs would strand every request.
    async fn build_command(
        program: &str,
        args: &[String],
        launch: &crate::mcp::sandbox::Launch,
        env: &[(String, String)],
        _notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Result<(Command, Option<Arc<crate::mcp::proxy::AllowListProxy>>), McpError> {
        match launch {
            crate::mcp::sandbox::Launch::Elevated => {
                let mut command = Command::new(program);
                command.args(args);
                for (key, value) in env {
                    command.env(key, value);
                }
                debug!("spawning plugin on the host (elevated): {program}");
                Ok((command, None))
            }
            crate::mcp::sandbox::Launch::Sandboxed(spec) => {
                let proxy = if spec.uses_proxy() {
                    let socket = spec.proxy_socket.clone().ok_or_else(|| {
                        McpError::Protocol(
                            "sandbox spec requests a proxy without a socket path".to_string(),
                        )
                    })?;
                    match crate::mcp::proxy::AllowListProxy::start(socket, spec.network.clone())
                        .await
                    {
                        Ok(proxy) => Some(proxy),
                        Err(e) => {
                            return Err(McpError::Protocol(format!(
                                "cannot start the sandbox allow-list proxy: {e}"
                            )));
                        }
                    }
                } else {
                    None
                };

                if !sandbox_available() {
                    return Err(McpError::Protocol(
                        "bubblewrap ('bwrap') is not installed, so this plugin cannot be run in \
                         isolation. Install bubblewrap, or set permissions.elevated=true in the \
                         plugin manifest to run it on the host instead."
                            .to_string(),
                    ));
                }

                let mut command = crate::mcp::sandbox::sandboxed_command(program, args, spec);
                for (key, value) in env {
                    command.env(key, value);
                }
                info!(
                    "spawning plugin in the sandbox: {}",
                    crate::mcp::sandbox::describe(spec)
                );
                // A plugin that needs the network but was granted nothing will
                // fail every outbound request with a bare reqwest error, which
                // reads like an upstream outage. Say so at spawn time, once.
                if spec.network.is_denied() {
                    warn!(
                        "plugin spawned with NO NETWORK: every outbound request will fail. \
                         Grant hosts in the plugin's Network allow-list \
                         (permissions.network.allow) if it needs the internet."
                    );
                }
                Ok((command, proxy))
            }
        }
    }

    pub async fn spawn(
        command_line: &str,
        env_raw: Option<&str>,
        agent_id: Option<&str>,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
    ) -> Result<Arc<Self>, McpError> {
        Self::spawn_with_sandbox(
            command_line,
            env_raw,
            agent_id,
            notification_tx,
            &crate::mcp::sandbox::Launch::Elevated,
        )
        .await
    }

    /// Spawn a plugin process under an explicit isolation policy.
    ///
    /// This is the entry point every host call site should use: it decides
    /// whether the plugin runs inside the bubblewrap sandbox
    /// ([`crate::mcp::sandbox`]) or, only when its manifest requested it, on the
    /// host.
    pub async fn spawn_with_sandbox(
        command_line: &str,
        env_raw: Option<&str>,
        agent_id: Option<&str>,
        notification_tx: mpsc::UnboundedSender<McpNotification>,
        launch: &crate::mcp::sandbox::Launch,
    ) -> Result<Arc<Self>, McpError> {
        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return Err(McpError::Protocol("Empty command string".to_string()));
        }

        let program = parts[0];
        let mut plugin_args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        let mut plugin_env: Vec<(String, String)> = Vec::new();

        if let Some(raw) = env_raw {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') || trimmed.starts_with('-') {
                plugin_args.extend(parse_cli_args(trimmed));
            } else if trimmed.starts_with('{') {
                plugin_env.extend(parse_env_vars(trimmed));
            } else if trimmed.lines().any(|l| l.contains('=') || l.contains(':')) {
                plugin_env.extend(parse_env_vars(trimmed));
            } else {
                plugin_args.extend(parse_cli_args(trimmed));
            }
        }

        // Plugins already receive `agent_id` on every tool call. Exporting it at
        // spawn time lets a plugin resolve per-agent state (tokens, channels)
        // before the first call, which background streams need.
        if let Some(agent_id) = agent_id {
            plugin_env.push(("NEUXDOT_AGENT_ID".to_string(), agent_id.to_string()));
        }

        let (mut command, proxy_guard) = Self::build_command(
            program,
            &plugin_args,
            launch,
            &plugin_env,
            notification_tx.clone(),
        )
        .await?;

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

        let transport = Arc::new(Self {
            stdin: Mutex::new(stdin),
            pending_requests,
            next_id: AtomicI64::new(1),
            child: Mutex::new(child),
            _proxy_guard: proxy_guard,
        });

        // Perform standard MCP initialization handshake (with 2s timeout for non-standard servers)
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "neuxdot",
                "version": "0.1.0"
            }
        });

        let init_future = transport.send_request("initialize", Some(init_params));
        if let Ok(Ok(_)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), init_future).await
        {
            let notif = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            let line = serde_json::to_string(&notif).unwrap_or_default() + "\n";
            let mut stdin_guard = transport.stdin.lock().await;
            let _ = stdin_guard.write_all(line.as_bytes()).await;
            let _ = stdin_guard.flush().await;
        }

        Ok(transport)
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
    use crate::db::schema::NetworkPermissions;
    use crate::mcp::sandbox::{Launch, SandboxSpec};

    /// Regression: the allow-list proxy used to be a local `_proxy_guard` that
    /// was dropped when `spawn_with_sandbox` returned. The sandboxed child then
    /// kept running with no proxy behind its `127.0.0.1:41873` relay, so every
    /// outbound request failed with "error sending request" — with the logs
    /// showing a *granted* network allow-list, which made it look like a
    /// Telegram outage. The transport must own the proxy for as long as the
    /// child lives.
    #[test]
    fn transport_holds_the_proxy_guard_for_the_childs_lifetime() {
        // The type of the field is the contract; this fails to compile if the
        // guard is ever removed or downgraded to a borrow.
        fn assert_owns_guard(t: &StdioTransport) -> &Option<Arc<crate::mcp::proxy::AllowListProxy>> {
            &t._proxy_guard
        }
        let _ = assert_owns_guard;
    }

    #[test]
    fn allow_list_launch_requests_a_proxy() {
        let spec = SandboxSpec {
            workspace: std::env::temp_dir().join("neuxdot-stdio-guard-test"),
            program: None,
            network: NetworkPermissions {
                allow: vec!["api.telegram.org".to_string()],
            },
            proxy_socket: Some(std::env::temp_dir().join("neuxdot-stdio-guard-test.sock")),
        };
        let launch = Launch::Sandboxed(spec);
        match launch {
            Launch::Sandboxed(s) => {
                assert!(s.uses_proxy(), "an allow-list must use the proxy");
                assert!(!s.shares_host_network(), "an allow-list must not get --share-net");
            }
            Launch::Elevated => panic!("allow-list must stay sandboxed"),
        }
    }


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
        assert_eq!(
            args,
            vec![
                "--token",
                "my-token",
                "--port",
                "8080",
                "--name",
                "My Service"
            ]
        );

        let json_raw = r#"["--token", "glpat-123", "--read-only"]"#;
        let args2 = parse_cli_args(json_raw);
        assert_eq!(args2, vec!["--token", "glpat-123", "--read-only"]);
    }
}
