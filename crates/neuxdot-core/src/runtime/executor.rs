use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};

use super::meta_tools::{
    calculate_memory_hash, get_meta_tools_definitions, interpolate_prompt, parse_duration_string,
    render_response_hooks, MAX_MEMORY_CHARS,
};
use super::router::ProviderRouter;
use super::timer_manager::global_timer_manager;
use crate::db::repos::Database;
use crate::db::schema::{ActiveModelTarget, Agent, AgentPushedMessage, McpServer};
use crate::events::{AgentEvent, EventEnvelope};
use crate::mcp::client::{McpClientSession, McpNotification, McpTool};
use crate::mcp::http::HttpTransport;
use crate::mcp::stdio::StdioTransport;
use crate::mcp::wasm_stdio::WasmStdioTransport;

pub struct AgentExecutor {
    pub db: Arc<Database>,
    pub agent: Agent,
    pub models: Vec<ActiveModelTarget>,
    pub mcp_servers: Vec<McpServer>,
    pub event_tx: broadcast::Sender<EventEnvelope>,
}

impl AgentExecutor {
    pub fn new(
        db: Arc<Database>,
        agent: Agent,
        models: Vec<ActiveModelTarget>,
        mcp_servers: Vec<McpServer>,
        event_tx: broadcast::Sender<EventEnvelope>,
    ) -> Self {
        Self {
            db,
            agent,
            models,
            mcp_servers,
            event_tx,
        }
    }

    fn emit(&self, event: AgentEvent) {
        let envelope = EventEnvelope::new(&self.agent.id, event);
        let _ = self.event_tx.send(envelope);
    }

    fn format_pushed_message(&self, msg: &AgentPushedMessage) -> String {
        let server_name = msg
            .mcp_id
            .as_deref()
            .and_then(|id| {
                self.mcp_servers
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.name.as_str())
            })
            .or_else(|| msg.mcp_id.as_deref())
            .unwrap_or("System MCP");

        let params_str = msg.params_json.as_deref().unwrap_or("{}");
        let parsed: serde_json::Value =
            serde_json::from_str(params_str).unwrap_or(serde_json::Value::Null);

        let uri_str = parsed.get("uri").and_then(|u| u.as_str());

        let time_str = msg
            .created_at
            .map(|t| t.format("%l:%M %p").to_string().trim().to_string())
            .unwrap_or_else(|| "just now".to_string());

        if let Some(uri) = uri_str {
            format!(
                "- [Source: {}] [Resource: {}] [Event: {}] ({}): {}\n",
                server_name, uri, msg.method, time_str, params_str
            )
        } else {
            format!(
                "- [Source: {}] [Event: {}] ({}): {}\n",
                server_name, msg.method, time_str, params_str
            )
        }
    }

    /// Initializes MCP sessions and sets up background listener for pushed information
    pub async fn setup_mcp_sessions(
        &self,
        wake_signal_tx: mpsc::UnboundedSender<String>,
    ) -> (
        HashMap<String, Arc<McpClientSession>>,
        HashMap<String, (String, McpTool)>,
    ) {
        let mut sessions = HashMap::new();
        let mut tool_registry = HashMap::new();

        for server in &self.mcp_servers {
            let (notif_tx, notif_rx) = mpsc::unbounded_channel::<McpNotification>();

            let transport: Option<Arc<dyn crate::mcp::client::McpTransport>> =
                if server.transport == "stdio" {
                    match StdioTransport::spawn(&server.url_or_command, notif_tx).await {
                        Ok(t) => Some(t),
                        Err(e) => {
                            self.emit(AgentEvent::AgentError {
                                agent_id: self.agent.id.clone(),
                                message: format!(
                                    "Failed to start MCP server '{}': {}",
                                    server.name, e
                                ),
                            });
                            None
                        }
                    }
                } else if server.transport == "wasm_stdio" || server.url_or_command.starts_with("wasm://") {
                    let ext_id = server.id.strip_prefix("wasm:").unwrap_or(&server.id);
                    match self.db.get_extension(ext_id).await {
                        Ok(Some(ext)) => {
                            let manifest_val: Value = serde_json::from_str(&ext.manifest_json).unwrap_or(Value::Null);
                            let data_dir = std::path::PathBuf::from("data");
                            Some(Arc::new(WasmStdioTransport::new(
                                Arc::clone(&self.db),
                                ext.id.clone(),
                                ext.wasm_hash.clone(),
                                manifest_val,
                                notif_tx,
                                data_dir,
                            )))
                        }
                        _ => {
                            warn!("WASM extension not found for MCP server: {}", server.name);
                            None
                        }
                    }
                } else {
                    Some(HttpTransport::new(&server.url_or_command, notif_tx))
                };

            if let Some(transport) = transport {
                let session = Arc::new(McpClientSession {
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    transport,
                    notification_rx: Mutex::new(notif_rx),
                });

                if let Err(e) = session.initialize().await {
                    warn!("MCP init handshake failed for {}: {}", server.name, e);
                }

                // Initial Subscriptions: send resources/subscribe for all agent subscriptions
                if let Ok(subscriptions) = self.db.get_agent_subscriptions(&self.agent.id).await {
                    for uri in &subscriptions {
                        let _ = session
                            .transport
                            .send_request("resources/subscribe", Some(serde_json::json!({ "uri": uri })))
                            .await;
                    }
                }

                // Granular Tool Permissions: filter out disabled tools from discovery
                let tool_perms = self
                    .db
                    .get_agent_tool_permissions(&self.agent.id)
                    .await
                    .unwrap_or_default();
                let disabled_tools: std::collections::HashSet<String> = tool_perms
                    .into_iter()
                    .filter(|p| p.mcp_id == server.id && !p.is_enabled)
                    .map(|p| p.tool_name)
                    .collect();

                match session.list_tools().await {
                    Ok(tools) => {
                        for tool in tools {
                            if disabled_tools.contains(&tool.name) {
                                info!(
                                    "Tool '{}' from MCP server '{}' is disabled for agent '{}' - hidden from discovery",
                                    tool.name, server.name, self.agent.name
                                );
                                continue;
                            }
                            tool_registry
                                .insert(tool.name.clone(), (server.id.clone(), tool));
                        }
                    }
                    Err(e) => {
                        warn!("Failed to list tools from {}: {}", server.name, e);
                    }
                }

                let agent_id = self.agent.id.clone();
                let mcp_id = server.id.clone();
                let mcp_name = server.name.clone();
                let event_tx = self.event_tx.clone();
                let db = Arc::clone(&self.db);
                let session_worker = Arc::clone(&session);
                let wake_tx = wake_signal_tx.clone();

                tokio::spawn(async move {
                    let mut rx = session_worker.notification_rx.lock().await;
                    while let Some(notif) = rx.recv().await {
                        info!(
                            "MCP server '{}' pushed information: method={}",
                            mcp_name, notif.method
                        );

                        let params_str = notif.params.as_ref().map(|p| p.to_string());
                        let _ = db
                            .save_pushed_message(&agent_id, Some(&mcp_id), &notif.method, params_str.as_deref())
                            .await;

                        let payload_val = notif.params.unwrap_or(Value::Null);
                        let envelope = EventEnvelope::new(
                            &agent_id,
                            AgentEvent::McpPushedNotification {
                                agent_id: agent_id.clone(),
                                mcp_id: mcp_id.clone(),
                                mcp_name: mcp_name.clone(),
                                method: notif.method.clone(),
                                payload: payload_val,
                            },
                        );
                        let _ = event_tx.send(envelope);

                        // Wake up interruptible sleep if sleeping
                        let _ = wake_tx.send(format!("MCP Push: {}", notif.method));
                    }
                });

                sessions.insert(server.id.clone(), session);
            }
        }

        (sessions, tool_registry)
    }

    /// Execute an agent turn using persistent running history and enforcing output text settings
    pub async fn run_turn(&self, user_input: &str) -> Result<String, String> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        self.emit(AgentEvent::TurnStarted {
            agent_id: self.agent.id.clone(),
            turn_id: turn_id.clone(),
        });

        // Cancel any active sleep timer for this agent so it doesn't retrigger or conflict
        global_timer_manager().cancel_timer(&self.agent.id, "new turn started");

        let (wake_tx, mut wake_rx) = mpsc::unbounded_channel::<String>();
        let (sessions, tool_registry) = self.setup_mcp_sessions(wake_tx).await;

        let mut allowed_tools: Vec<String> = tool_registry.keys().cloned().collect();
        allowed_tools.sort();

        // Fetch current memory
        let current_mem = self
            .db
            .get_agent_memory(&self.agent.id)
            .await
            .unwrap_or_else(|_| crate::db::schema::AgentMemory {
                agent_id: self.agent.id.clone(),
                content: String::new(),
                hash: calculate_memory_hash(""),
                updated_at: None,
            });

        // Load existing running history from SQLite
        let mut db_history = self
            .db
            .get_agent_history(&self.agent.id)
            .await
            .unwrap_or_default();

        let max_history_chars = self.agent.max_history_chars();
        if db_history.is_empty() {
            let base_template = self
                .agent
                .system_prompt
                .as_deref()
                .unwrap_or("You are {{agent.name}}, an autonomous AI assistant.");

            let system_text = interpolate_prompt(
                base_template,
                &self.agent.id,
                &self.agent.name,
                &self.agent.strategy,
                &current_mem.content,
                &current_mem.hash,
                &allowed_tools,
                max_history_chars,
            );

            let initial_sys = self
                .db
                .append_agent_history(&self.agent.id, "system", &system_text, None, None)
                .await
                .map_err(|e| e.to_string())?;

            db_history.push(initial_sys);
        }

        // Fetch and inject any unconsumed Push MCP updates / Subscriptions
        let unconsumed_pushed = self
            .db
            .list_unconsumed_pushed_messages(&self.agent.id)
            .await
            .unwrap_or_default();

        if !unconsumed_pushed.is_empty() {
            let mut pushed_context = String::from("### [Push MCP Updates / Subscriptions]:\n");
            let mut consumed_ids = Vec::new();
            for msg in &unconsumed_pushed {
                pushed_context.push_str(&self.format_pushed_message(msg));
                consumed_ids.push(msg.id.clone());
            }

            let pushed_item = self
                .db
                .append_agent_history(&self.agent.id, "system", &pushed_context, None, None)
                .await
                .map_err(|e| e.to_string())?;
            db_history.push(pushed_item);

            let _ = self.db.mark_pushed_messages_consumed(&self.agent.id, &consumed_ids).await;
        }

        // Append user message to database history
        let user_item = self
            .db
            .append_agent_history(&self.agent.id, "user", user_input, None, None)
            .await
            .map_err(|e| e.to_string())?;
        db_history.push(user_item);

        // Convert DB history items to OpenAI messages format
        let mut messages: Vec<Value> = db_history
            .iter()
            .map(|item| {
                let mut obj = json!({
                    "role": item.role,
                    "content": item.content,
                });
                if let Some(ref tc_str) = item.tool_calls_json {
                    if let Ok(tc_val) = serde_json::from_str::<Value>(tc_str) {
                        obj["tool_calls"] = tc_val;
                    }
                }
                if let Some(ref tid) = item.tool_call_id {
                    obj["tool_call_id"] = Value::String(tid.clone());
                }
                obj
            })
            .collect();

        let router = ProviderRouter::new(self.models.clone(), &self.agent.strategy);
        let meta_tools = get_meta_tools_definitions();

        let mut final_response = String::new();
        let max_iterations = 8;

        for _iteration in 0..max_iterations {
            let agent_id_for_event = self.agent.id.clone();
            let strategy_name = self.agent.strategy.clone();
            let event_tx_clone = self.event_tx.clone();

            let completion_result = router
                .execute_chat_completion(
                    messages.clone(),
                    Some(meta_tools.clone()),
                    move |target| {
                        let envelope = EventEnvelope::new(
                            &agent_id_for_event,
                            AgentEvent::ProviderSelected {
                                agent_id: agent_id_for_event.clone(),
                                provider_id: target.provider_id.clone(),
                                provider_name: target.provider_name.clone(),
                                model: target.model_id.clone(),
                                strategy: strategy_name.clone(),
                            },
                        );
                        let _ = event_tx_clone.send(envelope);
                    },
                )
                .await;

            let (response_json, _chosen_provider) = match completion_result {
                Ok(res) => res,
                Err(e) => {
                    let err_msg = format!("LLM completion failed: {}", e);
                    self.emit(AgentEvent::AgentError {
                        agent_id: self.agent.id.clone(),
                        message: err_msg.clone(),
                    });
                    return Err(err_msg);
                }
            };

            let choice = response_json["choices"]
                .as_array()
                .and_then(|arr| arr.first())
                .ok_or_else(|| "Empty choices array from LLM".to_string())?;

            let message = &choice["message"];
            let tool_calls = message.get("tool_calls").and_then(|tc| tc.as_array());
            let has_tool_calls = tool_calls.map(|tc| !tc.is_empty()).unwrap_or(false);
            let content_str = message["content"].as_str().unwrap_or("").trim();

            // Check Output Text Permission when no tool calls are made
            if !has_tool_calls {
                if !self.agent.allow_output_text {
                    let error_msg = self
                        .agent
                        .output_text_error_message
                        .as_deref()
                        .unwrap_or("Output Text is not allowed, consider use correct tool or sleep.");

                    self.emit(AgentEvent::TextDisallowed {
                        agent_id: self.agent.id.clone(),
                        error_message: error_msg.to_string(),
                    });

                    // Append assistant message, then feed rejection error message back to the LLM
                    messages.push(message.clone());
                    let user_rejection = json!({
                        "role": "user",
                        "content": error_msg
                    });
                    messages.push(user_rejection);

                    let _ = self
                        .db
                        .append_agent_history(
                            &self.agent.id,
                            "assistant",
                            content_str,
                            None,
                            None,
                        )
                        .await;

                    let _ = self
                        .db
                        .append_agent_history(
                            &self.agent.id,
                            "user",
                            error_msg,
                            None,
                            None,
                        )
                        .await;

                    continue;
                } else {
                    final_response = content_str.to_string();
                    messages.push(message.clone());
                    let _ = self
                        .db
                        .append_agent_history(
                            &self.agent.id,
                            "assistant",
                            content_str,
                            None,
                            None,
                        )
                        .await;
                    break;
                }
            }

            // Otherwise, assistant made tool calls
            messages.push(message.clone());
            let tool_calls_json_str = tool_calls.map(|tc| Value::Array(tc.clone()).to_string());
            let _ = self
                .db
                .append_agent_history(
                    &self.agent.id,
                    "assistant",
                    content_str,
                    tool_calls_json_str.as_deref(),
                    None,
                )
                .await;

            if let Some(tool_calls) = tool_calls {
                for call in tool_calls {
                    let call_id = call["id"].as_str().unwrap_or("call_0").to_string();
                    let fn_name = call["function"]["name"].as_str().unwrap_or("");
                    let fn_args_str = call["function"]["arguments"].as_str().unwrap_or("{}");
                    let fn_args: Value =
                        serde_json::from_str(fn_args_str).unwrap_or(json!({}));

                    self.emit(AgentEvent::ToolCallStarted {
                        agent_id: self.agent.id.clone(),
                        call_id: call_id.clone(),
                        tool_name: fn_name.to_string(),
                        arguments: fn_args.clone(),
                    });

                    let mut was_compacted = false;
                    let (result_value, success) = match fn_name {
                        "search_tools" => {
                            let query = fn_args["query"].as_str().unwrap_or("").to_lowercase();
                            let limit = fn_args["limit"].as_u64().unwrap_or(10) as usize;

                            let mut matches = vec![];
                            for (name, (_srv_id, tool)) in &tool_registry {
                                let desc = tool.description.as_deref().unwrap_or("").to_lowercase();
                                if name.to_lowercase().contains(&query) || desc.contains(&query) {
                                    matches.push(json!({
                                        "name": name,
                                        "description": tool.description
                                    }));
                                }
                                if matches.len() >= limit {
                                    break;
                                }
                            }

                            self.emit(AgentEvent::ToolSearch {
                                agent_id: self.agent.id.clone(),
                                query: query.clone(),
                                results_count: matches.len(),
                            });

                            (json!({ "tools": matches }), true)
                        }

                        "get_tool_schema" => {
                            let name = fn_args["name"].as_str().unwrap_or("");
                            self.emit(AgentEvent::ToolSchemaFetched {
                                agent_id: self.agent.id.clone(),
                                tool_name: name.to_string(),
                            });

                            if let Some((_srv_id, tool)) = tool_registry.get(name) {
                                (
                                    json!({
                                        "name": tool.name,
                                        "description": tool.description,
                                        "inputSchema": tool.input_schema
                                    }),
                                    true,
                                )
                            } else {
                                (json!({ "error": format!("Tool '{}' not found", name) }), false)
                            }
                        }

                        "call_tool" => {
                            let name = fn_args["name"].as_str().unwrap_or("");
                            let mut arguments = fn_args.get("arguments").cloned().unwrap_or(json!({}));

                            // If tool is chat_send, unconditionally route to this agent's actual ID
                            if name == "chat_send" {
                                if let Some(obj) = arguments.as_object_mut() {
                                    obj.insert("agent_id".to_string(), json!(self.agent.id));
                                }
                            }

                            if let Some((srv_id, _tool)) = tool_registry.get(name) {
                                if let Some(session) = sessions.get(srv_id) {
                                    match session.call_tool(name, arguments).await {
                                        Ok(res) => (res, true),
                                        Err(e) => (json!({ "error": e.to_string() }), false),
                                    }
                                } else {
                                    (
                                        json!({ "error": "MCP server session not active" }),
                                        false,
                                    )
                                }
                            } else {
                                (
                                    json!({ "error": format!("Tool '{}' not found", name) }),
                                    false,
                                )
                            }
                        }

                        "list_servers" => {
                            let list: Vec<Value> = self
                                .mcp_servers
                                .iter()
                                .map(|s| {
                                    json!({
                                        "id": s.id,
                                        "name": s.name,
                                        "transport": s.transport,
                                        "active": sessions.contains_key(&s.id)
                                    })
                                })
                                .collect();
                            (json!({ "servers": list }), true)
                        }

                        "setMemory" | "set_memory" => {
                            let content_str = fn_args["content"].as_str().unwrap_or("");
                            let char_count = content_str.chars().count();

                            if char_count > MAX_MEMORY_CHARS {
                                (
                                    json!({
                                        "error": format!(
                                            "Memory limit exceeded: new content has {} characters, exceeding the 5,000 character maximum limit. Rejected.",
                                            char_count
                                        )
                                    }),
                                    false,
                                )
                            } else {
                                let provided_hash = fn_args
                                    .get("currentHash")
                                    .or_else(|| fn_args.get("current_hash"))
                                    .and_then(|h| h.as_str())
                                    .unwrap_or("");

                                let mem = self.db.get_agent_memory(&self.agent.id).await.unwrap();

                                if mem.hash != provided_hash {
                                    (
                                        json!({
                                            "error": format!(
                                                "Hash mismatch: provided currentHash '{}' does not match actual memory hash '{}'. The memory may have been modified. Please call readMemory() to inspect current untruncated content and hash.",
                                                provided_hash, mem.hash
                                            )
                                        }),
                                        false,
                                    )
                                } else {
                                    let new_hash = calculate_memory_hash(content_str);
                                    let _ = self
                                        .db
                                        .set_agent_memory(&self.agent.id, content_str, &new_hash)
                                        .await;

                                    self.emit(AgentEvent::MemoryUpdated {
                                        agent_id: self.agent.id.clone(),
                                        updated_hash: new_hash.clone(),
                                        char_count,
                                    });

                                    (
                                        json!({
                                            "success": true,
                                            "updated_hash": new_hash,
                                            "character_count": char_count
                                        }),
                                        true,
                                    )
                                }
                            }
                        }

                        "editMemory" | "edit_memory" => {
                            let search = fn_args["search"].as_str().unwrap_or("");
                            let replace = fn_args["replace"].as_str().unwrap_or("");
                            let provided_hash = fn_args
                                .get("currentHash")
                                .or_else(|| fn_args.get("current_hash"))
                                .and_then(|h| h.as_str())
                                .unwrap_or("");

                            let mem = self.db.get_agent_memory(&self.agent.id).await.unwrap();

                            if mem.hash != provided_hash {
                                (
                                    json!({
                                        "error": format!(
                                            "Hash mismatch: provided currentHash '{}' does not match actual memory hash '{}'. The memory may have been modified. Please call readMemory() to inspect current untruncated content and hash.",
                                            provided_hash, mem.hash
                                        )
                                    }),
                                    false,
                                )
                            } else if !mem.content.contains(search) {
                                (
                                    json!({
                                        "error": format!(
                                            "Search segment '{}' not found in current memory.",
                                            search
                                        )
                                    }),
                                    false,
                                )
                            } else {
                                let new_content = mem.content.replace(search, replace);
                                let char_count = new_content.chars().count();

                                if char_count > MAX_MEMORY_CHARS {
                                    (
                                        json!({
                                            "error": format!(
                                                "Memory limit exceeded: resulting content has {} characters, exceeding the 5,000 character maximum limit. Rejected.",
                                                char_count
                                            )
                                        }),
                                        false,
                                    )
                                } else {
                                    let new_hash = calculate_memory_hash(&new_content);
                                    let _ = self
                                        .db
                                        .set_agent_memory(&self.agent.id, &new_content, &new_hash)
                                        .await;

                                    self.emit(AgentEvent::MemoryUpdated {
                                        agent_id: self.agent.id.clone(),
                                        updated_hash: new_hash.clone(),
                                        char_count,
                                    });

                                    (
                                        json!({
                                            "success": true,
                                            "updated_hash": new_hash,
                                            "character_count": char_count
                                        }),
                                        true,
                                    )
                                }
                            }
                        }

                        "readMemory" | "read_memory" => {
                            let mem = self.db.get_agent_memory(&self.agent.id).await.unwrap();
                            let char_count = mem.content.chars().count();

                            self.emit(AgentEvent::MemoryRead {
                                agent_id: self.agent.id.clone(),
                                current_hash: mem.hash.clone(),
                                char_count,
                            });

                            (
                                json!({
                                    "content": mem.content,
                                    "current_hash": mem.hash,
                                    "character_count": char_count
                                }),
                                true,
                            )
                        }

                        "compact" => {
                            let mem = self.db.get_agent_memory(&self.agent.id).await.unwrap();
                            let base_prompt = self
                                .agent
                                .system_prompt
                                .as_deref()
                                .unwrap_or("You are {{agent.name}}, an autonomous AI assistant.");

                            let compacted_system = interpolate_prompt(
                                base_prompt,
                                &self.agent.id,
                                &self.agent.name,
                                &self.agent.strategy,
                                &mem.content,
                                &mem.hash,
                                &allowed_tools,
                                max_history_chars,
                            );

                            let tool_reply = json!({
                                "success": true,
                                "message": "Running history compacted to system prompt.",
                                "current_hash": mem.hash
                            })
                            .to_string();

                            let _ = self
                                .db
                                .compact_agent_history(
                                    &self.agent.id,
                                    &compacted_system,
                                    Some(&call_id),
                                    Some(&tool_reply),
                                )
                                .await;

                            messages = vec![
                                json!({
                                    "role": "system",
                                    "content": compacted_system
                                }),
                                json!({
                                    "role": "tool",
                                    "tool_call_id": call_id,
                                    "content": tool_reply
                                }),
                            ];

                            was_compacted = true;

                            self.emit(AgentEvent::MemoryCompacted {
                                agent_id: self.agent.id.clone(),
                                current_hash: mem.hash.clone(),
                            });

                            (
                                json!({
                                    "success": true,
                                    "message": "Running history compacted to system prompt, memory, and currentHash.",
                                    "current_hash": mem.hash
                                }),
                                true,
                            )
                        }

                        "sleep" => {
                            let duration_str = fn_args
                                .get("duration")
                                .or_else(|| fn_args.get("time"))
                                .and_then(|d| d.as_str())
                                .unwrap_or("");

                            let interruptible = fn_args
                                .get("interruptible")
                                .and_then(|i| i.as_bool())
                                .unwrap_or(true);

                            match parse_duration_string(duration_str) {
                                Ok(dur) => {
                                    self.emit(AgentEvent::AgentSleeping {
                                        agent_id: self.agent.id.clone(),
                                        duration: duration_str.to_string(),
                                        interruptible,
                                    });

                                    let (cancel_tx, mut cancel_rx) =
                                        tokio::sync::oneshot::channel::<String>();
                                    // Strictly enforce single timer: cancelling any prior timer for this agent
                                    global_timer_manager().register_timer(
                                        &self.agent.id,
                                        cancel_tx,
                                        dur.as_secs(),
                                    );

                                    let was_interrupted: bool;
                                    let wake_reason: String;

                                    if interruptible {
                                        tokio::select! {
                                            _ = tokio::time::sleep(dur) => {
                                                global_timer_manager().clear_timer(&self.agent.id);
                                                was_interrupted = false;
                                                wake_reason = "timer_completed".to_string();
                                            }
                                            Ok(reason) = &mut cancel_rx => {
                                                was_interrupted = true;
                                                wake_reason = format!("interrupted: {}", reason);
                                            }
                                            Some(reason) = wake_rx.recv() => {
                                                was_interrupted = true;
                                                wake_reason = format!("interrupted by {}", reason);
                                                global_timer_manager().cancel_timer(&self.agent.id, &wake_reason);
                                            }
                                        }
                                    } else {
                                        tokio::select! {
                                            _ = tokio::time::sleep(dur) => {
                                                global_timer_manager().clear_timer(&self.agent.id);
                                                was_interrupted = false;
                                                wake_reason = "timer_completed".to_string();
                                            }
                                            Ok(reason) = &mut cancel_rx => {
                                                was_interrupted = true;
                                                wake_reason = format!("interrupted: {}", reason);
                                            }
                                        }
                                    }

                                    self.emit(AgentEvent::AgentWokeUp {
                                        agent_id: self.agent.id.clone(),
                                        reason: wake_reason.clone(),
                                    });

                                    (
                                        json!({
                                            "success": true,
                                            "duration": duration_str,
                                            "slept_seconds": dur.as_secs(),
                                            "interrupted": was_interrupted,
                                            "timer_cancelled": was_interrupted,
                                            "wake_reason": wake_reason
                                        }),
                                        true,
                                    )
                                }
                                Err(err_str) => (json!({ "error": err_str }), false),
                            }
                        }

                        other => (
                            json!({ "error": format!("Unknown meta-tool '{}'", other) }),
                            false,
                        ),
                    };

                    self.emit(AgentEvent::ToolCallCompleted {
                        agent_id: self.agent.id.clone(),
                        call_id: call_id.clone(),
                        tool_name: fn_name.to_string(),
                        result: result_value.clone(),
                        success,
                    });

                    if !was_compacted {
                        let current_chars: usize = messages.iter().map(|m| {
                            let content_len = m["content"].as_str().map(|s| s.chars().count()).unwrap_or(0);
                            let tc_len = m.get("tool_calls").map(|tc| tc.to_string().chars().count()).unwrap_or(0);
                            content_len + tc_len
                        }).sum();

                        let enabled_hooks = self.agent.get_response_hooks();
                        let rendered_hooks = render_response_hooks(&enabled_hooks, current_chars, max_history_chars);

                        let tool_content_str = if !rendered_hooks.is_empty() {
                            format!("{}\n\n[Response Hook]\n{}", result_value, rendered_hooks.join("\n"))
                        } else {
                            result_value.to_string()
                        };

                        let _ = self
                            .db
                            .append_agent_history(
                                &self.agent.id,
                                "tool",
                                &tool_content_str,
                                None,
                                Some(&call_id),
                            )
                            .await;

                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": tool_content_str
                        }));

                        // Enforce max_running_history_chars limit
                        let mut total_chars: usize = messages.iter().map(|m| {
                            let content_len = m["content"].as_str().map(|s| s.chars().count()).unwrap_or(0);
                            let tc_len = m.get("tool_calls").map(|tc| tc.to_string().chars().count()).unwrap_or(0);
                            content_len + tc_len
                        }).sum();

                        if total_chars > max_history_chars {
                            let _ = self.db.prune_agent_history_to_limit(&self.agent.id, max_history_chars).await;
                            while messages.len() > 1 && total_chars > max_history_chars {
                                let removed = messages.remove(1);
                                let rem_len = removed["content"].as_str().map(|s| s.chars().count()).unwrap_or(0);
                                let rem_tc = removed.get("tool_calls").map(|tc| tc.to_string().chars().count()).unwrap_or(0);
                                total_chars = total_chars.saturating_sub(rem_len + rem_tc);
                            }
                        }
                    }
                }
            }
        }

        self.emit(AgentEvent::TurnCompleted {
            agent_id: self.agent.id.clone(),
            turn_id,
            content: final_response.clone(),
        });

        Ok(final_response)
    }
}
