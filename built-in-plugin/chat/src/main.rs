use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
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
        assert_eq!(arr.len(), 4);
        let names: Vec<&str> = arr.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"chat_send"));
        assert!(names.contains(&"chat_user_message"));
        assert!(names.contains(&"chat_history"));
        assert!(names.contains(&"chat_clear"));
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
