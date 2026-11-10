mod sandbox_fs;

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

const CHAT_APP_HTML: &str = include_str!("app.html");
const DATA_DIR: &str = "data/chat";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" or "assistant"
    pub text: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
}

struct AppState {
    // In-memory cache of agent conversations
    history: Mutex<HashMap<String, Vec<ChatMessage>>>,
    active_agent_id: Mutex<Option<String>>,
    stdout_lock: Mutex<()>,
    storage_dir: PathBuf,
}

impl AppState {
    fn new() -> Self {
        let storage_dir = PathBuf::from(DATA_DIR);
        let _ = fs::create_dir_all(&storage_dir);
        Self {
            history: Mutex::new(HashMap::new()),
            active_agent_id: Mutex::new(None),
            stdout_lock: Mutex::new(()),
            storage_dir,
        }
    }

    async fn send_json(&self, val: &Value) {
        let _guard = self.stdout_lock.lock().await;
        if let Ok(line) = serde_json::to_string(val) {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            let _ = out.flush();
        }
    }

    async fn send_notification(&self, method: &str, params: Value) {
        self.send_json(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await;
    }

    fn file_path_for(&self, agent_id: &str) -> PathBuf {
        let safe_id = agent_id.replace(['/', '\\', ':', '.'], "_");
        self.storage_dir.join(format!("history_{}.json", safe_id))
    }

    async fn get_messages(&self, agent_id: &str) -> Vec<ChatMessage> {
        let mut map = self.history.lock().await;
        if let Some(msgs) = map.get(agent_id) {
            return msgs.clone();
        }

        // Try reading from file
        let path = self.file_path_for(agent_id);
        if path.exists() {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(msgs) = serde_json::from_str::<Vec<ChatMessage>>(&raw) {
                    map.insert(agent_id.to_string(), msgs.clone());
                    return msgs;
                }
            }
        }

        Vec::new()
    }

    async fn save_messages(&self, agent_id: &str, msgs: &[ChatMessage]) {
        let mut map = self.history.lock().await;
        map.insert(agent_id.to_string(), msgs.to_vec());

        let path = self.file_path_for(agent_id);
        if let Ok(raw) = serde_json::to_string(msgs) {
            let _ = fs::write(&path, raw);
        }
    }

    async fn clear_messages(&self, agent_id: &str) {
        let mut map = self.history.lock().await;
        map.remove(agent_id);

        let path = self.file_path_for(agent_id);
        let _ = fs::remove_file(path);
    }
}

fn current_time_str() -> String {
    Local::now().format("%l:%M %p").to_string().trim().to_string()
}

fn get_tool_definitions() -> Value {
    json!([
        {
            "name": "chat_send",
            "description": "Send an assistant reply message into the interactive chat console",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Optional target agent ID (defaults to active agent)"
                    },
                    "message": {
                        "type": "string",
                        "description": "The message text to send into the chat console"
                    }
                },
                "required": ["message"]
            },
            "_meta": {
                "ui": {
                    "resourceUri": "ui://chat-console"
                }
            }
        },
        {
            "name": "chat_user_message",
            "description": "Send a user message from the MCP-App UI and store in history",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Target agent ID"
                    },
                    "message": {
                        "type": "string",
                        "description": "User prompt message"
                    }
                },
                "required": ["agent_id", "message"]
            },
            "_meta": {
                "ui": {
                    "resourceUri": "ui://chat-console",
                    "visibility": ["app"]
                }
            }
        },
        {
            "name": "chat_history",
            "description": "Retrieve conversation history for the specified agent",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Target agent ID"
                    }
                },
                "required": ["agent_id"]
            },
            "_meta": {
                "ui": {
                    "resourceUri": "ui://chat-console",
                    "visibility": ["app"]
                }
            }
        },
        {
            "name": "chat_clear",
            "description": "Clear conversation history for the specified agent",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Target agent ID"
                    }
                },
                "required": ["agent_id"]
            },
            "_meta": {
                "ui": {
                    "resourceUri": "ui://chat-console",
                    "visibility": ["app"]
                }
            }
        },
        {
            "name": "chat_upload_file",
            "description": "Store a file in this agent's sandbox workspace and return where it landed. Base64-encode the bytes; the model never needs to hold file contents in its context twice. The file is then reachable from chat_exec at the returned path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Destination path inside /workspace, e.g. notes/report.md. Parent directories are created."
                    },
                    "content_base64": {
                        "type": "string",
                        "description": "File bytes, standard base64. Mutually exclusive with 'content'."
                    },
                    "content": {
                        "type": "string",
                        "description": "File bytes as UTF-8 text, for text files. Mutually exclusive with 'content_base64'."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "Replace an existing file (default: false, which returns an error instead)"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "chat_list_files",
            "description": "List files and directories inside this agent's sandbox workspace, with sizes and modification times.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to list, relative to /workspace (default: the workspace root)"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Walk subdirectories (default: false, which lists one level)"
                    }
                }
            }
        },
        {
            "name": "chat_read_file",
            "description": "Read a file from this agent's sandbox workspace. Text is returned directly; binary content is returned as base64 when 'encoding' is 'base64'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path relative to /workspace"
                    },
                    "encoding": {
                        "type": "string",
                        "enum": ["text", "base64"],
                        "description": "How to return the bytes (default: text)"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Read at most this many bytes (default 262144, capped at 8388608)"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "chat_exec",
            "description": "Run a bash command inside the sandboxed workspace. No network, read-only /usr, and only /workspace is writable. Output is captured and truncated with an explicit marker rather than silently cut.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Bash command to run; starts in /workspace"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Wall-clock limit (default 30, max 300)"
                    }
                },
                "required": ["command"]
            }
        }
    ])
}

fn get_resource_definitions() -> Value {
    json!([
        {
            "uri": "ui://chat-console",
            "name": "Interactive Chat",
            "description": "Sandboxed MCP-App interactive chat interface",
            "mimeType": "text/html;profile=mcp-app"
        },
        {
            "uri": "extension://chat/messages",
            "name": "Live Chat Messages Stream",
            "description": "Real-time updates of messages from user chat console (Push MCP Subscriptions)",
            "mimeType": "application/json"
        }
    ])
}

// -----------------------------------------------------------------------------
// Tool Implementations
// -----------------------------------------------------------------------------

async fn tool_chat_send(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'message' parameter".to_string())?
        .trim();

    if message.is_empty() {
        return Err("message is required and cannot be empty".to_string());
    }

    let mut agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if agent_id.is_empty() {
        let active = state.active_agent_id.lock().await;
        agent_id = active.clone().unwrap_or_else(|| "default".to_string());
    }

    let mut messages = state.get_messages(&agent_id).await;
    let bot_msg = ChatMessage {
        id: format!("ast_{}", messages.len() + 1),
        role: "assistant".to_string(),
        text: message.to_string(),
        timestamp: current_time_str(),
        sender_id: None,
    };
    messages.push(bot_msg);

    state.save_messages(&agent_id, &messages).await;

    // Emit reactive push notifications
    state
        .send_notification(
            "notifications/resources/updated",
            json!({ "uri": "ui://chat-console" }),
        )
        .await;

    state
        .send_notification(
            "notifications/resources/updated",
            json!({
                "uri": "extension://chat/messages",
                "agent_id": agent_id,
                "role": "assistant",
                "text": message
            }),
        )
        .await;

    Ok(json!({
        "success": true,
        "message": format!("Message posted to chat console for agent '{}'.", agent_id),
        "count": messages.len()
    }))
}

async fn tool_chat_user_message(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'agent_id' parameter".to_string())?
        .trim()
        .to_string();

    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'message' parameter".to_string())?
        .trim();

    if message.is_empty() {
        return Err("message cannot be empty".to_string());
    }

    // Remember active agent
    {
        let mut active = state.active_agent_id.lock().await;
        *active = Some(agent_id.clone());
    }

    let sender_id = args
        .get("_sender_account_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());

    let mut messages = state.get_messages(&agent_id).await;
    let user_msg = ChatMessage {
        id: format!("usr_{}", messages.len() + 1),
        role: "user".to_string(),
        text: message.to_string(),
        timestamp: current_time_str(),
        sender_id,
    };
    messages.push(user_msg);

    state.save_messages(&agent_id, &messages).await;

    // Emit reactive push notifications
    state
        .send_notification(
            "notifications/resources/updated",
            json!({ "uri": "ui://chat-console" }),
        )
        .await;

    state
        .send_notification(
            "notifications/resources/updated",
            json!({
                "uri": "extension://chat/messages",
                "agent_id": agent_id,
                "role": "user",
                "text": message
            }),
        )
        .await;

    Ok(json!(messages))
}

async fn tool_chat_history(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'agent_id' parameter".to_string())?
        .trim();

    let messages = state.get_messages(agent_id).await;
    Ok(json!(messages))
}

async fn tool_chat_clear(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'agent_id' parameter".to_string())?
        .trim();

    state.clear_messages(agent_id).await;

    state
        .send_notification(
            "notifications/resources/updated",
            json!({ "uri": "ui://chat-console" }),
        )
        .await;

    Ok(json!({
        "success": true,
        "message": format!("Chat history cleared for agent '{}'.", agent_id)
    }))
}

// -----------------------------------------------------------------------------
// Sandboxed workspace tools
// -----------------------------------------------------------------------------

/// The agent's workspace root inside the plugin's own data directory.
///
/// This directory is bind-mounted read-write into the plugin's bubblewrap
/// sandbox as `/workspace` and nowhere else, so a path resolved here can never
/// address the host.
fn agent_workspace(state: &AppState, agent_id: &str) -> PathBuf {
    let safe: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    state.storage_dir.join("agents").join(safe).join("workspace")
}

async fn resolve_agent_id(state: &Arc<AppState>, args: &Value) -> String {
    if let Some(id) = args.get("agent_id").and_then(|v| v.as_str()) {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let active = state.active_agent_id.lock().await;
    active.clone().unwrap_or_else(|| "default".to_string())
}

async fn tool_chat_upload_file(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = resolve_agent_id(state, args).await;
    let root = agent_workspace(state, &agent_id);
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'path' parameter".to_string())?;
    let target = sandbox_fs::resolve_in_root(&root, raw_path)?;

    let bytes: Vec<u8> = match (
        args.get("content_base64").and_then(|v| v.as_str()),
        args.get("content").and_then(|v| v.as_str()),
    ) {
        (Some(b64), None) => sandbox_fs::decode_base64(b64)?,
        (None, Some(text)) => text.as_bytes().to_vec(),
        (Some(_), Some(_)) => {
            return Err(
                "Pass either 'content_base64' or 'content', not both".to_string()
            );
        }
        (None, None) => {
            return Err("One of 'content' or 'content_base64' is required".to_string());
        }
    };

    if bytes.len() > sandbox_fs::MAX_UPLOAD_BYTES {
        return Err(format!(
            "File is {} bytes; the limit is {} bytes",
            bytes.len(),
            sandbox_fs::MAX_UPLOAD_BYTES
        ));
    }

    let overwrite = args
        .get("overwrite")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if target.exists() && !overwrite {
        return Err(format!(
            "'{raw_path}' already exists. Pass overwrite=true to replace it."
        ));
    }

    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Cannot create the parent directory: {e}"))?;
    }
    tokio::fs::write(&target, &bytes)
        .await
        .map_err(|e| format!("Cannot write '{raw_path}': {e}"))?;

    let mount_path = target
        .strip_prefix(&root)
        .map(|p| format!("/workspace/{}", p.display()))
        .unwrap_or_else(|_| raw_path.to_string());

    Ok(json!({
        "success": true,
        "path": mount_path,
        "bytes": bytes.len(),
        "message": format!("Stored {} bytes at {}.", bytes.len(), mount_path)
    }))
}

async fn tool_chat_list_files(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = resolve_agent_id(state, args).await;
    let root = agent_workspace(state, &agent_id);
    let relative = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let dir = if relative.trim().is_empty() {
        root.clone()
    } else {
        sandbox_fs::resolve_in_root(&root, relative)?
    };

    let recursive = args
        .get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !dir.exists() {
        return Ok(json!({ "success": true, "path": relative, "entries": [] }));
    }

    let mut entries = Vec::new();
    collect_entries(&root, &dir, recursive, 0, &mut entries).await?;

    Ok(json!({
        "success": true,
        "path": if relative.trim().is_empty() { "/workspace" } else { relative },
        "count": entries.len(),
        "entries": entries
    }))
}

/// Walk `dir`, describing each child relative to `root`.
///
/// Depth-bounded so a symlink loop cannot hang the plugin; the sandbox already
/// has no route out, but a plugin must not be able to wedge itself either.
async fn collect_entries(
    root: &Path,
    dir: &Path,
    recursive: bool,
    depth: usize,
    out: &mut Vec<Value>,
) -> Result<(), String> {
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return Ok(());
    }

    let mut reader = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("Cannot read '{}': {e}", dir.display()))?;

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| format!("Cannot read '{}': {e}", dir.display()))?
    {
        let path = entry.path();
        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let relative = path
            .strip_prefix(root)
            .map(|p| format!("/workspace/{}", p.display()))
            .unwrap_or_else(|_| path.display().to_string());
        let is_dir = metadata.is_dir();
        out.push(json!({
            "path": relative,
            "name": entry.file_name().to_string_lossy(),
            "type": if is_dir { "dir" } else if metadata.is_file() { "file" } else { "other" },
            "bytes": if metadata.is_file() { metadata.len() } else { 0 },
        }));

        if recursive && is_dir {
            Box::pin(collect_entries(root, &path, true, depth + 1, out)).await?;
        }
    }
    Ok(())
}

async fn tool_chat_read_file(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = resolve_agent_id(state, args).await;
    let root = agent_workspace(state, &agent_id);
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'path' parameter".to_string())?;
    let target = sandbox_fs::resolve_in_root(&root, raw_path)?;

    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| format!("Cannot stat '{raw_path}': {e}"))?;
    if metadata.is_dir() {
        return Err(format!("'{raw_path}' is a directory; use chat_list_files"));
    }

    let requested = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(262_144)
        .clamp(1, sandbox_fs::MAX_UPLOAD_BYTES as u64) as usize;

    let bytes = tokio::fs::read(&target)
        .await
        .map_err(|e| format!("Cannot read '{raw_path}': {e}"))?;
    let truncated = bytes.len() > requested;
    let slice = &bytes[..bytes.len().min(requested)];

    let encoding = args
        .get("encoding")
        .and_then(|v| v.as_str())
        .unwrap_or("text");

    let mut out = json!({
        "success": true,
        "path": raw_path,
        "bytes": bytes.len(),
        "truncated": truncated,
    });
    if encoding.eq_ignore_ascii_case("base64") {
        out["encoding"] = json!("base64");
        out["content"] = json!(sandbox_fs::encode_base64(slice));
    } else {
        out["encoding"] = json!("text");
        out["content"] = json!(sandbox_fs::clamp_output(slice));
    }
    Ok(out)
}

async fn tool_chat_exec(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let agent_id = resolve_agent_id(state, args).await;
    let workspace = agent_workspace(state, &agent_id);
    tokio::fs::create_dir_all(&workspace)
        .await
        .map_err(|e| format!("Cannot create the workspace: {e}"))?;

    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'command' parameter".to_string())?
        .trim()
        .to_string();
    if command.is_empty() {
        return Err("'command' cannot be empty".to_string());
    }

    let timeout_secs = sandbox_fs::clamp_timeout(
        args.get("timeout_seconds").and_then(|v| v.as_u64()),
    );

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc")
        .arg(&command)
        .current_dir(&workspace)
        // Inherit the sandbox's environment, which the host already configured,
        // but never the caller's cwd.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let started = std::time::Instant::now();
    let child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start the command: {e}"))?;

    let output = match tokio::time::timeout(
        sandbox_fs::timeout_duration(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(format!("The command failed to run: {e}")),
        Err(_) => {
            return Ok(json!({
                "success": false,
                "timed_out": true,
                "timeout_seconds": timeout_secs,
                "message": format!("Command exceeded {timeout_secs}s and was killed."),
            }));
        }
    };

    let elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(json!({
        "success": output.status.success(),
        "exit_code": output.status.code(),
        "timed_out": false,
        "duration_ms": elapsed_ms,
        "cwd": "/workspace",
        "stdout": sandbox_fs::clamp_output(&output.stdout),
        "stderr": sandbox_fs::clamp_output(&output.stderr),
    }))
}

// -----------------------------------------------------------------------------
// Request Dispatcher
// -----------------------------------------------------------------------------

async fn handle_request(state: &Arc<AppState>, req: Value) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (no 'id')
    if id.is_none() {
        if method == "notifications/initialized" {
            eprintln!("[chat-mcp] Client initialized notification received");
        }
        return;
    }

    let req_id = id.unwrap();

    match method {
        "initialize" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {
                            "subscribe": true
                        }
                    },
                    "serverInfo": {
                        "name": "chat-mcp",
                        "version": "2.0.0"
                    }
                }
            });
            state.send_json(&res).await;
        }

        "resources/subscribe" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {}
            });
            state.send_json(&res).await;
        }

        "resources/unsubscribe" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {}
            });
            state.send_json(&res).await;
        }

        "ping" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {}
            });
            state.send_json(&res).await;
        }

        "tools/list" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "tools": get_tool_definitions()
                }
            });
            state.send_json(&res).await;
        }

        "resources/list" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "resources": get_resource_definitions()
                }
            });
            state.send_json(&res).await;
        }

        "resources/read" => {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            match uri {
                "ui://chat-console" => {
                    let res = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "contents": [
                                {
                                    "uri": "ui://chat-console",
                                    "mimeType": "text/html;profile=mcp-app",
                                    "text": CHAT_APP_HTML
                                }
                            ]
                        }
                    });
                    state.send_json(&res).await;
                }
                "extension://chat/messages" => {
                    let agent_id = params
                        .get("agent_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    let messages = state.get_messages(agent_id).await;
                    let text_json = serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string());
                    let res = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "contents": [
                                {
                                    "uri": "extension://chat/messages",
                                    "mimeType": "application/json",
                                    "text": text_json
                                }
                            ]
                        }
                    });
                    state.send_json(&res).await;
                }
                other => {
                    let res = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": {
                            "code": -32602,
                            "message": format!("Resource '{}' not found", other)
                        }
                    });
                    state.send_json(&res).await;
                }
            }
        }

        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let result: Result<Value, String> = match tool_name {
                "chat_send" => tool_chat_send(state, &arguments).await,
                "chat_user_message" => tool_chat_user_message(state, &arguments).await,
                "chat_history" => tool_chat_history(state, &arguments).await,
                "chat_clear" => tool_chat_clear(state, &arguments).await,
                "chat_upload_file" => tool_chat_upload_file(state, &arguments).await,
                "chat_list_files" => tool_chat_list_files(state, &arguments).await,
                "chat_read_file" => tool_chat_read_file(state, &arguments).await,
                "chat_exec" => tool_chat_exec(state, &arguments).await,
                other => Err(format!("Unknown tool: {}", other)),
            };

            let response_body = match result {
                Ok(val) => {
                    let text_content = if val.is_string() {
                        val.as_str().unwrap().to_string()
                    } else {
                        val.to_string()
                    };
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": text_content
                                }
                            ],
                            "isError": false
                        }
                    })
                }
                Err(err_msg) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": err_msg
                                }
                            ],
                            "isError": true
                        }
                    })
                }
            };

            state.send_json(&response_body).await;
        }

        other => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {}", other)
                }
            });
            state.send_json(&res).await;
        }
    }
}

// -----------------------------------------------------------------------------
// Main Loop
// -----------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(AppState::new());
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut tasks = tokio::task::JoinSet::new();

    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(trimmed) {
            Ok(req) => {
                let state_clone = Arc::clone(&state);
                tasks.spawn(async move {
                    handle_request(&state_clone, req).await;
                });
            }
            Err(e) => {
                eprintln!("[chat-mcp] Invalid JSON received: {} (error: {})", trimmed, e);
                let err_res = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                state.send_json(&err_res).await;
            }
        }
    }

    // Drain in-flight tasks before closing
    while tasks.join_next().await.is_some() {}

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions() {
        let defs = get_tool_definitions();
        let arr = defs.as_array().expect("array of tools");

        // The exact tool set is an interface, so it is pinned rather than
        // counted: a tool silently disappearing would otherwise pass.
        let mut names: Vec<&str> = arr.iter().filter_map(|t| t["name"].as_str()).collect();
        names.sort_unstable();
        let expected = [
            "chat_clear",
            "chat_exec",
            "chat_history",
            "chat_list_files",
            "chat_read_file",
            "chat_send",
            "chat_upload_file",
            "chat_user_message",
        ];
        assert_eq!(names, expected);

        // Every tool must carry a usable schema, not just a name.
        for tool in arr {
            assert!(
                tool["inputSchema"]["type"] == "object",
                "tool {:?} has no object inputSchema",
                tool["name"]
            );
        }
    }

    #[test]
    fn test_resource_definitions() {
        let defs = get_resource_definitions();
        let arr = defs.as_array().expect("array of resources");
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().any(|r| r["uri"] == "ui://chat-console"));
    }

    #[tokio::test]
    async fn test_chat_message_flow() {
        let state = Arc::new(AppState::new());
        let agent_id = "test-agent-123";

        // 1. User message
        let user_res = tool_chat_user_message(
            &state,
            &json!({ "agent_id": agent_id, "message": "Hello agent!" }),
        )
        .await
        .expect("user message succeeds");
        assert!(user_res.as_array().unwrap().len() >= 1);

        // 2. Assistant message
        let bot_res = tool_chat_send(
            &state,
            &json!({ "agent_id": agent_id, "message": "Hello human!" }),
        )
        .await
        .expect("assistant message succeeds");
        assert_eq!(bot_res["success"], true);

        // 3. History
        let hist_res = tool_chat_history(&state, &json!({ "agent_id": agent_id }))
            .await
            .expect("history succeeds");
        let hist_arr = hist_res.as_array().unwrap();
        assert_eq!(hist_arr.len(), 2);
        assert_eq!(hist_arr[0]["role"], "user");
        assert_eq!(hist_arr[0]["text"], "Hello agent!");
        assert_eq!(hist_arr[1]["role"], "assistant");
        assert_eq!(hist_arr[1]["text"], "Hello human!");

        // 4. Clear
        let clear_res = tool_chat_clear(&state, &json!({ "agent_id": agent_id }))
            .await
            .expect("clear succeeds");
        assert_eq!(clear_res["success"], true);

        let hist_empty = tool_chat_history(&state, &json!({ "agent_id": agent_id }))
            .await
            .expect("history succeeds");
        assert_eq!(hist_empty.as_array().unwrap().len(), 0);
    }
}
