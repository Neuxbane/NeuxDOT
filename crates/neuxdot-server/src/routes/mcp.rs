use axum::{
    extract::{Path, State},
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(servers))
}

pub async fn create_mcp_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateMcpRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let transport = payload.transport.to_lowercase();
    if transport != "stdio" && transport != "http" && transport != "custom_tools" && transport != "curl" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Transport must be 'stdio', 'http', or 'custom_tools'" })),
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    if !is_active {
        let _ = state.db.set_plugin_active(&server.id, false).await;
    }

    if let Some(ref perms) = payload.permissions {
        let perms_str = serde_json::to_string(perms).unwrap_or_else(|_| "{}".to_string());
        let _ = state.db.update_plugin_permissions(&server.id, &perms_str).await;
    }

    let final_server = state
        .db
        .get_mcp_server(&server.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Plugin not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
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
        .update_mcp_server(
            &id,
            name,
            alias,
            &transport,
            url_or_command,
            headers_or_env,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(updated))
}

/// Execute a JSON-RPC 2.0 request against any Plugin (`wasm`, `stdio`, or `http`).
/// Powers the standard MCP-Apps bidirectional bridge (`resources/read`, `tools/call`, etc.).
pub async fn plugin_rpc_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PluginRpcRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = state.db.sync_wasm_mcp_servers(&auth.account_id).await;

    let server_opt = state
        .db
        .get_mcp_server(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let server = match server_opt {
        Some(s) => s,
        None => {
            let wasm_prefixed = format!("wasm:{}", id);
            state
                .db
                .get_mcp_server(&wasm_prefixed)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
                .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Plugin not found" }))))?
        }
    };

    if server.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    if !server.is_active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Plugin is currently disabled" })),
        ));
    }

    let perms = server.get_permissions();
    if payload.method == "resources/read" {
        if let Some(uri) = payload.params.as_ref().and_then(|p| p.get("uri")).and_then(|u| u.as_str()) {
            // Per-resource UI allow-list (docs/HOST_BRIDGE.md §2): a plugin may
            // render only the `ui://` resources it was granted.
            if uri.starts_with("ui://") && !perms.allows_ui_resource(uri) {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": format!("UI resource '{}' is not granted for this plugin", uri) })),
                ));
            }
        }
    }

    let (notif_tx, _notif_rx) =
        tokio::sync::mpsc::unbounded_channel::<neuxdot_core::mcp::client::McpNotification>();

    let transport: Arc<dyn McpTransport> = if server.is_wasm() {
        let ext_id = server.id.strip_prefix("wasm:").unwrap_or(&server.id);
        let ext = state
            .db
            .get_extension(ext_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "WASM plugin binary not found" }))))?;

        let manifest_val: Value = serde_json::from_str(&ext.manifest_json).unwrap_or(Value::Null);
        let data_dir = std::path::PathBuf::from("data/extensions");
        let _ = std::fs::create_dir_all(&data_dir);

        Arc::new(neuxdot_core::mcp::wasm_stdio::WasmStdioTransport::new(
            Arc::clone(&state.db),
            ext.id,
            ext.wasm_hash,
            manifest_val,
            notif_tx,
            data_dir,
        ))
    } else if server.transport == "custom_tools" || server.transport == "curl" {
        Arc::new(
            neuxdot_core::mcp::custom_tools::CustomToolsTransport::new(
                &server.id,
                &server.url_or_command,
                notif_tx,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))))?,
        )
    } else if server.transport == "stdio" {
        if !perms.process {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Process spawn permission is disabled for this stdio plugin" })),
            ));
        }
        neuxdot_core::mcp::stdio::StdioTransport::spawn(
            &server.url_or_command,
            server.headers_or_env.as_deref(),
            notif_tx,
        )
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))))?
    } else {
        if !perms.network {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Network permission is disabled for this HTTP plugin" })),
            ));
        }
        neuxdot_core::mcp::http::HttpTransport::new(
            &server.url_or_command,
            server.headers_or_env.as_deref(),
            notif_tx,
        )
    };

    let result = transport
        .send_request(&payload.method, payload.params.clone())
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))))?;

    let _ = transport.close().await;

    // Emit push notification when user sends a message via MCP-App chat_user_message
    if payload.method == "tools/call" {
        if let Some(params) = &payload.params {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if tool_name == "chat_user_message" {
                let args = params.get("arguments");
                let agent_id = args
                    .and_then(|a| a.get("agent_id"))
                    .and_then(|v| v.as_str())
                    .or(payload.agent_id.as_deref());
                let user_msg = args
                    .and_then(|a| a.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if let Some(aid) = agent_id {
                    let notif_params = json!({
                        "uri": "extension://chat/messages",
                        "role": "user",
                        "text": user_msg
                    });
                    let notif_str = notif_params.to_string();
                    let _ = state
                        .db
                        .save_pushed_message(
                            aid,
                            Some(&server.id),
                            "notifications/resources/updated",
                            Some(&notif_str),
                        )
                        .await;

                    let envelope = EventEnvelope::new(
                        aid,
                        AgentEvent::McpPushedNotification {
                            agent_id: aid.to_string(),
                            mcp_id: server.id.clone(),
                            mcp_name: server.name.clone(),
                            method: "notifications/resources/updated".to_string(),
                            payload: notif_params,
                        },
                    );
                    let _ = state.event_tx.send(envelope);
                }
            }
        }
    }

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "MCP server not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let deleted = state
        .db
        .delete_mcp_server(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "MCP server not found" }))))?;

    if server.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    // Find target agent(s)
    let agent_ids: Vec<String> = if let Some(target) = payload.target_agent_id {
        vec![target]
    } else {
        // Broadcast to all agents belonging to this account
        let agents = state
            .db
            .list_agents(&auth.account_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
        agents.into_iter().map(|a| a.id).collect()
    };

    let params_val = payload.params.unwrap_or(serde_json::Value::Null);
    let params_str = params_val.to_string();

    for agent_id in &agent_ids {
        // 1. Save to database for agent awareness
        let _ = state
            .db
            .save_pushed_message(agent_id, Some(&server.id), &payload.method, Some(&params_str))
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
