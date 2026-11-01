use serde::{Deserialize, Serialize};

// --- Host Imports ---
#[link(wasm_import_module = "env")]
extern "C" {
    fn host_log(msg_ptr: *const u8, msg_len: usize);
    fn host_kv_get(key_ptr: *const u8, key_len: usize, out_ptr: *mut u8, max_out: usize) -> i32;
    fn host_kv_set(key_ptr: *const u8, key_len: usize, val_ptr: *const u8, val_len: usize) -> i32;
    fn host_kv_delete(key_ptr: *const u8, key_len: usize) -> i32;
    fn host_fs_write(path_ptr: *const u8, path_len: usize, data_ptr: *const u8, data_len: usize) -> i32;
    #[allow(dead_code)]
    fn host_fs_read(path_ptr: *const u8, path_len: usize, out_ptr: *mut u8, max_out: usize) -> i32;
    #[allow(dead_code)]
    fn host_agent_run(
        agent_id_ptr: *const u8,
        agent_id_len: usize,
        input_ptr: *const u8,
        input_len: usize,
        out_ptr: *mut u8,
        max_out: usize,
    ) -> i32;
    fn host_agent_run_background(
        agent_id_ptr: *const u8,
        agent_id_len: usize,
        input_ptr: *const u8,
        input_len: usize,
    ) -> i32;
    fn host_get_time_str(out_ptr: *mut u8, max_out: usize) -> i32;
}

fn log(msg: &str) {
    unsafe {
        host_log(msg.as_ptr(), msg.len());
    }
}

fn get_current_time() -> String {
    let mut buf = vec![0u8; 64];
    let len = unsafe { host_get_time_str(buf.as_mut_ptr(), buf.len()) };
    if len > 0 {
        buf.truncate(len as usize);
        String::from_utf8(buf).unwrap_or_else(|_| String::from("12:00 PM"))
    } else {
        String::from("12:00 PM")
    }
}

#[allow(dead_code)]
fn agent_run(agent_id: &str, input: &str) -> String {
    let mut buf = vec![0u8; 65536];
    let len = unsafe {
        host_agent_run(
            agent_id.as_ptr(),
            agent_id.len(),
            input.as_ptr(),
            input.len(),
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    if len >= 0 {
        buf.truncate(len as usize);
        String::from_utf8(buf).unwrap_or_else(|_| String::from("Failed to decode agent response"))
    } else {
        String::from("Agent execution failed")
    }
}

fn kv_get(key: &str) -> Option<String> {
    let mut buf = vec![0u8; 65536]; // 64KB max read
    let len = unsafe {
        host_kv_get(key.as_ptr(), key.len(), buf.as_mut_ptr(), buf.len())
    };
    if len >= 0 {
        buf.truncate(len as usize);
        String::from_utf8(buf).ok()
    } else {
        None
    }
}

fn kv_set(key: &str, val: &str) -> bool {
    let res = unsafe {
        host_kv_set(key.as_ptr(), key.len(), val.as_ptr(), val.len())
    };
    res == 0
}

fn kv_delete(key: &str) -> bool {
    let res = unsafe {
        host_kv_delete(key.as_ptr(), key.len())
    };
    res == 0
}

fn fs_write(path: &str, data: &[u8]) -> bool {
    let res = unsafe {
        host_fs_write(path.as_ptr(), path.len(), data.as_ptr(), data.len())
    };
    res == 0
}

// Memory allocator for host to pass data into WASM
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr
}

fn pack_string_response(s: String) -> i64 {
    let bytes = s.into_bytes();
    let ptr = bytes.as_ptr() as usize;
    let len = bytes.len();
    core::mem::forget(bytes);
    ((ptr as i64) << 32) | (len as i64)
}

#[derive(Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" or "assistant"
    pub text: String,
    pub timestamp: String,
}

#[derive(Deserialize)]
pub struct SendMessagePayload {
    pub agent_id: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Deserialize)]
pub struct AgentPayload {
    pub agent_id: String,
}

/// 1. Return extension manifest JSON with UI hooks and auto-exposed MCP Server
#[no_mangle]
pub extern "C" fn get_manifest() -> i64 {
    let manifest = r#"{
  "id": "community.agent-chat",
  "name": "Agent Chat Console",
  "version": "1.0.0",
  "description": "Standardized interactive chat console for agents, powered by host-managed isolated KV, file storage, and auto-exposed MCP stdio tools.",
  "ui_hooks": [
    {
      "target_id": "agent-sidebar-nav",
      "type": "agent_panel",
      "panel_id": "chat-console",
      "title": "Interactive Chat",
      "icon": "MessageSquare"
    }
  ],
  "mcp": {
    "server_name": "Agent Chat Console [WASM]",
    "tools": [
      {
        "name": "chat_send",
        "description": "Send a chat message directly into the interactive chat console",
        "inputSchema": {
          "type": "object",
          "properties": {
            "message": { "type": "string", "description": "The message to send into the chat console" }
          },
          "required": ["message"]
        }
      }
    ],
    "resources": [
      {
        "uri": "extension://chat/messages",
        "name": "Live Chat Messages Stream",
        "description": "Real-time updates of messages from user chat console (Push MCP Subscriptions)",
        "mimeType": "application/json"
      }
    ]
  },
  "data_access": [
    "agent.id",
    "agent.name"
  ],
  "network": {
    "hosts": []
  }
}"#;
    pack_string_response(String::from(manifest))
}

/// 2. Execute dynamic actions
#[no_mangle]
pub extern "C" fn execute_action(
    action_ptr: *const u8,
    action_len: usize,
    payload_ptr: *const u8,
    payload_len: usize,
) -> i64 {
    let action = unsafe {
        core::str::from_utf8(core::slice::from_raw_parts(action_ptr, action_len)).unwrap_or("")
    };
    let payload = unsafe {
        core::str::from_utf8(core::slice::from_raw_parts(payload_ptr, payload_len)).unwrap_or("")
    };

    match action {
        "get_history" => {
            if let Ok(p) = serde_json::from_str::<AgentPayload>(payload) {
                let key = format!("chat_history_{}", p.agent_id);
                let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                // Ensure only non-empty messages are returned
                if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&history_json) {
                    let valid_messages: Vec<ChatMessage> = messages
                        .into_iter()
                        .filter(|m| !m.text.trim().is_empty())
                        .collect();
                    if let Ok(clean_json) = serde_json::to_string(&valid_messages) {
                        return pack_string_response(clean_json);
                    }
                }
                return pack_string_response(history_json);
            }
            pack_string_response(String::from("[]"))
        }

        "clear_history" => {
            if let Ok(p) = serde_json::from_str::<AgentPayload>(payload) {
                let key = format!("chat_history_{}", p.agent_id);
                kv_delete(&key);
                log(&format!("Cleared chat history for agent {}", p.agent_id));
                return pack_string_response(String::from(r#"{"status":"cleared"}"#));
            }
            pack_string_response(String::from(r#"{"error":"invalid payload"}"#))
        }

        "send_message" => {
            if let Ok(p) = serde_json::from_str::<SendMessagePayload>(payload) {
                let trimmed_msg = p.message.trim();
                if trimmed_msg.is_empty() {
                    return pack_string_response(String::from(r#"{"error":"empty message"}"#));
                }

                let key = format!("chat_history_{}", p.agent_id);
                let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                let mut messages: Vec<ChatMessage> = serde_json::from_str(&history_json).unwrap_or_else(|_| vec![]);

                let time_str = p.timestamp.unwrap_or_else(get_current_time);

                // 1. Add user message
                let user_msg = ChatMessage {
                    id: format!("usr_{}", messages.len() + 1),
                    role: String::from("user"),
                    text: trimmed_msg.to_string(),
                    timestamp: time_str,
                };
                messages.push(user_msg);

                // 2. Persist immediately to host-managed isolated KV
                if let Ok(updated_json) = serde_json::to_string(&messages) {
                    kv_set(&key, &updated_json);
                    kv_set("active_agent_id", &p.agent_id);

                    // Write audit log line
                    let log_entry = format!("User {}: {}\n", p.agent_id, trimmed_msg);
                    fs_write(&format!("transcript_{}.log", p.agent_id), log_entry.as_bytes());

                    // 3. Dispatch agent asynchronously in background — DOES NOT BLOCK USER!
                    unsafe {
                        host_agent_run_background(
                            p.agent_id.as_ptr(),
                            p.agent_id.len(),
                            trimmed_msg.as_ptr(),
                            trimmed_msg.len(),
                        );
                    }

                    log(&format!("Saved turn for agent {}. Dispatched background agent run.", p.agent_id));
                    return pack_string_response(updated_json);
                }
            }
            pack_string_response(String::from(r#"{"error":"failed to process message"}"#))
        }

        "mcp_call_tool" => {
            #[derive(Deserialize)]
            struct McpCallToolPayload {
                name: String,
                arguments: serde_json::Value,
            }
            if let Ok(call) = serde_json::from_str::<McpCallToolPayload>(payload) {
                match call.name.as_str() {
                    "chat_send" => {
                        let mut agent_id = call.arguments.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        let message = call.arguments.get("message").and_then(|v| v.as_str()).unwrap_or("").trim();
                        if message.is_empty() {
                            let err = serde_json::json!({
                                "isError": true,
                                "content": [{ "type": "text", "text": "message is required and cannot be empty" }]
                            });
                            return pack_string_response(err.to_string());
                        }

                        // Robust fallback: if agent_id is not a UUID (e.g. LLM supplied agent name or omitted),
                        // use active_agent_id from KV:
                        if !agent_id.contains('-') || agent_id.starts_with("community.") {
                            if let Some(active) = kv_get("active_agent_id") {
                                if !active.is_empty() {
                                    agent_id = active;
                                }
                            }
                        }

                        let key = format!("chat_history_{}", agent_id);
                        let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                        let mut messages: Vec<ChatMessage> = serde_json::from_str(&history_json).unwrap_or_else(|_| vec![]);

                        let bot_msg = ChatMessage {
                            id: format!("ast_{}", messages.len() + 1),
                            role: String::from("assistant"),
                            text: message.to_string(),
                            timestamp: get_current_time(),
                        };
                        messages.push(bot_msg);

                        if let Ok(updated_json) = serde_json::to_string(&messages) {
                            kv_set(&key, &updated_json);
                            let log_entry = format!("Agent {} (chat_send): {}\n", agent_id, message);
                            fs_write(&format!("transcript_{}.log", agent_id), log_entry.as_bytes());
                        }

                        let reply = serde_json::json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Message posted to chat console for agent '{}'.", agent_id)
                                }
                            ]
                        });
                        return pack_string_response(reply.to_string());
                    }
                    _ => {
                        let err = serde_json::json!({
                            "isError": true,
                            "content": [{ "type": "text", "text": format!("Unknown tool '{}'", call.name) }]
                        });
                        return pack_string_response(err.to_string());
                    }
                }
            }
            pack_string_response(String::from(r#"{"isError":true,"content":[{"type":"text","text":"Invalid payload for mcp_call_tool"}]}"#))
        }

        _ => pack_string_response(format!(r#"{{"error":"unknown action {}"}}"#, action)),
    }
}
