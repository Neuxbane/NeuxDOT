use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use neuxdot_core::events::{AgentEvent, EventEnvelope};
use neuxdot_core::mcp::client::McpTransport;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateMcpRequest {
    pub name: String,
    pub alias: Option<String>,
    pub transport: String, // "stdio", "http", or "custom_tools"
    pub url_or_command: String,
    pub headers_or_env: Option<String>,
    pub is_active: Option<bool>,
    pub permissions: Option<Value>,
}

#[derive(Deserialize)]
pub struct UpdateMcpRequest {
    pub name: Option<String>,
    pub alias: Option<String>,
    pub transport: Option<String>,
    pub url_or_command: Option<String>,
    pub headers_or_env: Option<String>,
    pub is_active: Option<bool>,
    pub permissions: Option<Value>,
}

#[derive(Deserialize)]
pub struct PluginRpcRequest {
    pub method: String,
    pub params: Option<Value>,
    pub agent_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PushMcpMessageRequest {
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub target_agent_id: Option<String>,
}

pub async fn list_mcp_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let servers = state
        .db
        .list_mcp_servers(&auth.account_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(servers))
}

/// `GET /api/plugins/:id` (and `/api/mcp/:id`)
///
/// Returns one plugin. The edit modal needs this to load a single custom tool
/// definition; without it the UI fell back to a 405 and the page rendered a 500.
pub async fn get_mcp_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let server = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Plugin not found" })),
            )
        })?;

    if server.account_id != auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied" })),
        ));
    }

    Ok(Json(server))
}

pub async fn create_mcp_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateMcpRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let transport = payload.transport.to_lowercase();
    if transport != "stdio"
        && transport != "binary"
        && transport != "http"
        && transport != "custom_tools"
        && transport != "curl"
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "Transport must be 'stdio', 'binary', 'http', or 'custom_tools'" }),
            ),
        ));
    }

    let is_active = payload.is_active.unwrap_or(true);

    let server = state
        .db
        .create_mcp_server(
            &auth.account_id,
            &payload.name,
            payload.alias.as_deref(),
            &transport,
            &payload.url_or_command,
            payload.headers_or_env.as_deref(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    if !is_active {
        let _ = state.db.set_plugin_active(&server.id, false).await;
    }

    if let Some(ref perms) = payload.permissions {
        let perms_str = serde_json::to_string(perms).unwrap_or_else(|_| "{}".to_string());
        let _ = state
            .db
            .update_plugin_permissions(&server.id, &perms_str)
            .await;
    }

    let final_server = state
        .db
        .get_mcp_server(&server.id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .unwrap_or(server);

    Ok((StatusCode::CREATED, Json(final_server)))
}

pub async fn update_mcp_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateMcpRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Plugin not found" })),
            )
        })?;

    if existing.account_id != auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied" })),
        ));
    }

    if let Some(active) = payload.is_active {
        let _ = state.db.set_plugin_active(&id, active).await;
    }

    if let Some(ref perms) = payload.permissions {
        let perms_str = serde_json::to_string(perms).unwrap_or_else(|_| "{}".to_string());
        let _ = state.db.update_plugin_permissions(&id, &perms_str).await;
    }

    let name = payload.name.as_deref().unwrap_or(&existing.name);
    let alias = payload.alias.as_deref().or(existing.alias.as_deref());
    let transport = payload
        .transport
        .as_deref()
        .unwrap_or(&existing.transport)
        .to_lowercase();
    let url_or_command = payload
        .url_or_command
        .as_deref()
        .unwrap_or(&existing.url_or_command);
    let headers_or_env = payload
        .headers_or_env
        .as_deref()
        .or(existing.headers_or_env.as_deref());

    let updated = state
        .db
        .update_mcp_server(&id, name, alias, &transport, url_or_command, headers_or_env)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    // A running push stream was spawned with the *old* permissions, command and
    // env. Reconcile every agent that binds this plugin so a permission edit
    // (e.g. adding a network allow-list) actually takes effect, instead of the
    // old no-network sandbox silently polling forever.
    if let Ok(agent_ids) = state.db.list_agent_ids_for_mcp(&id).await {
        for agent_id in agent_ids {
            state.subscriptions.sync_agent(&agent_id).await;
        }
    }

    Ok(Json(updated))
}

/// Execute a JSON-RPC 2.0 request against any Plugin (`binary`, `stdio`, or `http`).
/// Powers the standard MCP-Apps bidirectional bridge (`resources/read`, `tools/call`, etc.).
pub async fn plugin_rpc_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PluginRpcRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let server = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Plugin not found" })),
            )
        })?;

    // A plugin belongs to the account that installed it, but it must also be
    // usable by anyone the owning agent is shared with.
    if server.account_id != auth.account_id {
        let allowed = match payload.agent_id.as_deref() {
            Some(agent_id) => match state.db.get_agent(agent_id).await {
                Ok(Some(agent)) => state
                    .db
                    .resolve_agent_access(&agent, &auth.account_id)
                    .await
                    .map(|a| a.can_view())
                    .unwrap_or(false),
                _ => false,
            },
            None => false,
        };
        if !allowed {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Access denied" })),
            ));
        }
    }

    if !server.is_active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Plugin is currently disabled" })),
        ));
    }

    let perms = server.get_permissions();
    if payload.method == "resources/read" {
        if let Some(uri) = payload
            .params
            .as_ref()
            .and_then(|p| p.get("uri"))
            .and_then(|u| u.as_str())
        {
            // Per-resource UI allow-list (docs/HOST_BRIDGE.md §2): a plugin may
            // render only the `ui://` resources it was granted.
            if uri.starts_with("ui://") && !perms.allows_ui_resource(uri) {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(
                        json!({ "error": format!("UI resource '{}' is not granted for this plugin", uri) }),
                    ),
                ));
            }
        }
    }

    let (notif_tx, mut notif_rx) =
        tokio::sync::mpsc::unbounded_channel::<neuxdot_core::mcp::client::McpNotification>();

    let effective_headers_or_env = if let Some(ref agent_id) = payload.agent_id {
        let override_opt = state
            .db
            .get_agent_mcp_config(agent_id, &server.id)
            .await
            .unwrap_or(None);
        neuxdot_core::db::repos::merge_headers_or_env(
            server.headers_or_env.as_deref(),
            override_opt.as_deref(),
        )
    } else {
        server.headers_or_env.clone()
    };

    let transport: Arc<dyn McpTransport> = if server.transport == "custom_tools"
        || server.transport == "curl"
    {
        Arc::new(
            neuxdot_core::mcp::custom_tools::CustomToolsTransport::new(
                &server.id,
                &server.url_or_command,
                notif_tx,
            )
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": e.to_string() })),
                )
            })?,
        )
    } else if server.transport == "stdio" || server.transport == "binary" {
        if !perms.process {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Process spawn permission is disabled for this plugin" })),
            ));
        }
        let launch = neuxdot_core::mcp::sandbox::SandboxSpec::launch_for(&server);
        neuxdot_core::mcp::stdio::StdioTransport::spawn_with_sandbox(
            &server.url_or_command,
            effective_headers_or_env.as_deref(),
            payload.agent_id.as_deref(),
            notif_tx,
            &launch,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )
        })?
    } else {
        if perms.network.is_denied() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Network permission is disabled for this HTTP plugin" })),
            ));
        }
        neuxdot_core::mcp::http::HttpTransport::new(
            &server.url_or_command,
            effective_headers_or_env.as_deref(),
            notif_tx,
        )
    };

    // Stamp the *trusted* caller identity onto tool arguments so a plugin can
    // attribute messages to whoever actually sent them (the UI cannot spoof it).
    // Underscore-prefixed keys are host metadata, never model-facing schema.
    // Stamp trusted caller identity; see `mcp::stamp_sender`.
    let forwarded_params = neuxdot_core::mcp::stamp_sender(
        &payload.method,
        payload.params.clone(),
        &auth.account_id,
        &auth.username,
    );

    let result = transport
        .send_request(&payload.method, forwarded_params)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    // Allow trailing push notifications emitted by the tool call to arrive
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    // Drain any reactive push notifications emitted by the plugin (e.g. chat console user messages)
    while let Ok(notif) = notif_rx.try_recv() {
        let target_agent_id = notif
            .params
            .as_ref()
            .and_then(|p| p.get("agent_id"))
            .and_then(|a| a.as_str())
            .map(|s| s.to_string())
            .or_else(|| payload.agent_id.clone())
            .or_else(|| {
                payload
                    .params
                    .as_ref()
                    .and_then(|p| p.get("arguments"))
                    .and_then(|args| args.get("agent_id"))
                    .and_then(|a| a.as_str())
                    .map(|s| s.to_string())
            });

        let target_ids: Vec<String> = if let Some(target) = target_agent_id {
            vec![target]
        } else {
            state
                .db
                .list_agents(&auth.account_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.id)
                .collect()
        };

        let params_val = notif.params.unwrap_or(serde_json::Value::Null);
        let params_str = params_val.to_string();

        for agent_id in &target_ids {
            let _ = state
                .db
                .save_pushed_message(agent_id, Some(&server.id), &notif.method, Some(&params_str))
                .await;

            let envelope = EventEnvelope::new(
                agent_id,
                AgentEvent::McpPushedNotification {
                    agent_id: agent_id.clone(),
                    mcp_id: server.id.clone(),
                    mcp_name: server.name.clone(),
                    method: notif.method.clone(),
                    payload: params_val.clone(),
                },
            );
            let _ = state.event_tx.send(envelope);

            // If this push notification is a user chat message from the chat console,
            // trigger an agent turn automatically so the agent replies into the chat console
            let is_user_chat_message = notif.method == "notifications/resources/updated"
                && params_val.get("uri").and_then(|u| u.as_str())
                    == Some("extension://chat/messages")
                && params_val.get("role").and_then(|r| r.as_str()) == Some("user");

            if is_user_chat_message {
                let agent_id_clone = agent_id.clone();
                let state_clone = state.clone();
                let user_text = params_val
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                tokio::spawn(async move {
                    if let Ok(Some(agent)) = state_clone.db.get_agent(&agent_id_clone).await {
                        let models = state_clone
                            .db
                            .get_agent_models(&agent.id)
                            .await
                            .unwrap_or_default();
                        let mcp_servers = state_clone
                            .db
                            .get_agent_mcp_servers(&agent.id)
                            .await
                            .unwrap_or_default();
                        let executor = neuxdot_core::runtime::executor::AgentExecutor::new(
                            Arc::clone(&state_clone.db),
                            agent,
                            models,
                            mcp_servers,
                            state_clone.event_tx.clone(),
                        );
                        let prompt = if user_text.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "User message from chat console: \"{}\". Respond using the chat_send tool.",
                                user_text
                            )
                        };
                        let _ = executor.run_turn(&prompt).await;
                    }
                });
            }
        }
    }

    let _ = transport.close().await;

    Ok(Json(result))
}

pub async fn delete_mcp_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "MCP server not found" })),
            )
        })?;

    if existing.account_id != auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied" })),
        ));
    }

    let deleted = state.db.delete_mcp_server(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    Ok(Json(json!({ "deleted": deleted })))
}

/// Push information from an MCP server directly into agent(s)
pub async fn push_mcp_information_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PushMcpMessageRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let server = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "MCP server not found" })),
            )
        })?;

    if server.account_id != auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied" })),
        ));
    }

    // Find target agent(s)
    let agent_ids: Vec<String> = if let Some(target) = payload.target_agent_id {
        vec![target]
    } else {
        // Broadcast to all agents belonging to this account
        let agents = state.db.list_agents(&auth.account_id).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        agents.into_iter().map(|a| a.id).collect()
    };

    let params_val = payload.params.unwrap_or(serde_json::Value::Null);
    let params_str = params_val.to_string();

    for agent_id in &agent_ids {
        // 1. Save to database for agent awareness
        let _ = state
            .db
            .save_pushed_message(
                agent_id,
                Some(&server.id),
                &payload.method,
                Some(&params_str),
            )
            .await;

        // 2. Broadcast live event to terminal
        let envelope = EventEnvelope::new(
            agent_id,
            AgentEvent::McpPushedNotification {
                agent_id: agent_id.clone(),
                mcp_id: server.id.clone(),
                mcp_name: server.name.clone(),
                method: payload.method.clone(),
                payload: params_val.clone(),
            },
        );
        let _ = state.event_tx.send(envelope);
    }

    Ok(Json(json!({
        "status": "pushed",
        "affected_agents_count": agent_ids.len()
    })))
}

#[derive(Debug, Deserialize)]
pub struct TestCustomToolPayload {
    pub tool: neuxdot_core::mcp::custom_tools::CustomToolDef,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub session_variables: std::collections::HashMap<String, String>,
}

pub async fn test_custom_tool_handler(
    _auth: AuthUser,
    State(_state): State<AppState>,
    Json(payload): Json<TestCustomToolPayload>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match neuxdot_core::mcp::custom_tools::CustomToolsTransport::execute_standalone(
        &payload.tool,
        &payload.arguments,
        &payload.session_variables,
    )
    .await
    {
        Ok((status, body, updated_vars)) => Ok(Json(json!({
            "success": status >= 200 && status < 300,
            "status": status,
            "body": body,
            "updated_session_variables": updated_vars
        }))),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e, "success": false })),
        )),
    }
}

/// Write `bytes` to `target` in a way that survives the target being executed.
///
/// A direct `fs::write` onto a running executable fails with `ETXTBSY`
/// ("Text file busy"), because the kernel refuses to open for writing an inode
/// that a process is executing. That is the normal case here: re-uploading a
/// plugin binary while an agent has it bound is exactly how a plugin is
/// updated.
///
/// The fix is the standard atomic-replace dance:
///
/// 1. Write the bytes to a unique temporary file **in the same directory**, so
///    the final `rename` stays on one filesystem and is therefore atomic.
/// 2. Apply the executable bit to the temporary file, so the target is never
///    observable in a non-executable state.
/// 3. `rename` it over the target. Renaming swaps the directory entry while any
///    running process keeps its already-open inode, so `ETXTBSY` cannot occur
///    and the running plugin is undisturbed.
///
/// A crash between steps leaves a stray `.tmp-*` file, never a truncated binary:
/// a partially written upload can no longer corrupt a working plugin.
pub fn write_binary_atomically(target: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = target.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;

    // Unique per process and per call: two concurrent uploads must not share a
    // temporary file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "plugin".to_string());
    let tmp_path = parent.join(format!(
        ".tmp-{}-{seq}-{stamp}-{file_name}",
        std::process::id()
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(bytes)?;
        // Flush to the filesystem before the rename, so a power loss cannot
        // leave the *new* name pointing at unwritten data.
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Set the mode on the temporary file, never on a live target: the
        // plugin is executable the instant its new name appears.
        if let Err(err) =
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
        {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
    }

    if let Err(err) = std::fs::rename(&tmp_path, target) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

pub async fn upload_binary_handler(
    _auth: AuthUser,
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut raw_filename: Option<String> = None;

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or_default().to_string();
                if name == "file" || name == "binary" {
                    if let Some(fname) = field.file_name() {
                        raw_filename = Some(fname.to_string());
                    }
                    let data = field.bytes().await.map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": format!("Failed to read multipart stream: {}", e) })),
                        )
                    })?;
                    file_bytes = Some(data.to_vec());
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("Multipart error: {}", e) })),
                ));
            }
        }
    }

    let bytes = file_bytes.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "No binary file provided in multipart upload (expected field 'file')" })),
        )
    })?;

    let default_name = "custom_mcp_binary";
    let base_name = raw_filename
        .as_deref()
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(default_name)
        })
        .unwrap_or(default_name);

    let sanitized: String = base_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    let sanitized = if sanitized.is_empty() {
        default_name.to_string()
    } else {
        sanitized
    };

    let bin_dir = std::path::PathBuf::from("data/binaries");
    let _ = std::fs::create_dir_all(&bin_dir);

    let target_path = bin_dir.join(&sanitized);

    write_binary_atomically(&target_path, &bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to write binary file: {}", e) })),
        )
    })?;

    let rel_path = format!("./data/binaries/{}", sanitized);
    let stem = std::path::Path::new(&sanitized)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&sanitized);

    let words: Vec<String> = stem
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect();

    let suggested_name = if words.is_empty() {
        "Custom Binary Plugin".to_string()
    } else {
        words.join(" ")
    };

    let suggested_alias = stem.to_lowercase().replace(['-', ' '], "_");

    Ok(Json(json!({
        "path": rel_path,
        "filename": sanitized,
        "size": bytes.len(),
        "suggested_name": suggested_name,
        "suggested_alias": suggested_alias,
    })))
}
