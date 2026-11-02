use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use neuxdot_core::runtime::executor::AgentExecutor;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AgentToolItem {
    pub mcp_id: String,
    pub server_name: String,
    pub server_alias: String,
    pub name: String,
    pub unique_name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Serialize)]
pub struct AgentResourceItem {
    pub mcp_id: String,
    pub server_name: String,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub is_subscribed: bool,
}

#[derive(Deserialize)]
pub struct UpdateToolPermissionRequest {
    pub mcp_id: String,
    pub tool_name: String,
    pub is_enabled: bool,
}

#[derive(Deserialize)]
pub struct UpdateSubscriptionsRequest {
    pub subscriptions: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
    pub avatar_url: Option<String>,
    pub strategy: Option<String>,
    pub system_prompt: Option<String>,
    pub provider_ids: Option<Vec<String>>,
    pub model_ids: Option<Vec<String>>,
    pub mcp_ids: Option<Vec<String>>,
    pub allow_output_text: Option<bool>,
    pub output_text_error_message: Option<String>,
    pub max_running_history_chars: Option<i64>,
    pub response_hooks_json: Option<String>,
    pub response_hooks: Option<Vec<serde_json::Value>>,
    /// Selected Agent Instructions preset (omitted = Custom / free-text).
    pub instruction_preset_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PatchAgentRequest {
    pub name: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
    pub avatar_url: Option<String>,
    pub strategy: Option<String>,
    pub system_prompt: Option<String>,
    pub is_active: Option<bool>,
    pub provider_ids: Option<Vec<String>>,
    pub model_ids: Option<Vec<String>>,
    pub mcp_ids: Option<Vec<String>>,
    pub allow_output_text: Option<bool>,
    pub output_text_error_message: Option<String>,
    pub max_running_history_chars: Option<i64>,
    pub response_hooks_json: Option<String>,
    pub response_hooks: Option<Vec<serde_json::Value>>,
    /// Selected Agent Instructions preset. Send `""` (or explicit null) to go
    /// back to Custom; omit to leave the selection untouched.
    pub instruction_preset_id: Option<String>,
}

#[derive(Deserialize)]
pub struct RunAgentRequest {
    pub input: String,
}

pub async fn list_agents_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agents = state
        .db
        .list_agents(&auth.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(agents))
}

pub async fn get_agent_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let bound_models = state
        .db
        .get_agent_models(&agent.id)
        .await
        .unwrap_or_default();
    let bound_mcp = state
        .db
        .get_agent_mcp_servers(&agent.id)
        .await
        .unwrap_or_default();

    let model_ids: Vec<String> = bound_models.iter().map(|m| m.id.clone()).collect();

    Ok(Json(json!({
        "agent": agent,
        "model_ids": model_ids.clone(),
        "provider_ids": model_ids,
        "models": bound_models,
        "mcp_ids": bound_mcp.iter().map(|m| &m.id).collect::<Vec<_>>(),
    })))
}

pub async fn create_agent_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateAgentRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let hooks_json = if let Some(ref hooks) = payload.response_hooks {
        Some(serde_json::to_string(hooks).unwrap_or_else(|_| "[]".to_string()))
    } else {
        payload.response_hooks_json
    };

    let agent = state
        .db
        .create_agent(
            &auth.account_id,
            &payload.name,
            payload.avatar_color.as_deref(),
            payload.avatar_shape.as_deref(),
            payload.avatar_url.as_deref(),
            payload.strategy.as_deref().unwrap_or("fallback"),
            payload.system_prompt.as_deref(),
            payload.allow_output_text,
            payload.output_text_error_message.as_deref(),
            payload.max_running_history_chars,
            hooks_json.as_deref(),
            payload.instruction_preset_id.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let target_models = payload.model_ids.or(payload.provider_ids);
    if let Some(ref mids) = target_models {
        let _ = state.db.set_agent_models(&agent.id, mids).await;
    }

    if let Some(ref mids) = payload.mcp_ids {
        let _ = state.db.set_agent_mcp_servers(&agent.id, mids).await;
    }

    Ok((StatusCode::CREATED, Json(agent)))
}

pub async fn patch_agent_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PatchAgentRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let hooks_json = if let Some(ref hooks) = payload.response_hooks {
        Some(serde_json::to_string(hooks).unwrap_or_else(|_| "[]".to_string()))
    } else {
        payload.response_hooks_json
    };

    let updated = state
        .db
        .patch_agent(
            &id,
            payload.name.as_deref(),
            payload.avatar_color.as_deref(),
            payload.avatar_shape.as_deref(),
            payload.avatar_url.as_deref(),
            payload.strategy.as_deref(),
            payload.system_prompt.as_deref(),
            payload.is_active,
            payload.allow_output_text,
            payload.output_text_error_message.as_deref(),
            payload.max_running_history_chars,
            hooks_json.as_deref(),
            payload.instruction_preset_id.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let target_models = payload.model_ids.or(payload.provider_ids);
    if let Some(ref mids) = target_models {
        let _ = state.db.set_agent_models(&id, mids).await;
    }

    if let Some(ref mids) = payload.mcp_ids {
        let _ = state.db.set_agent_mcp_servers(&id, mids).await;
    }

    Ok(Json(updated))
}

pub async fn delete_agent_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let deleted = state
        .db
        .delete_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "deleted": deleted })))
}

pub async fn run_agent_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<RunAgentRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let models = state
        .db
        .get_agent_models(&agent.id)
        .await
        .unwrap_or_default();
    let mcp_servers = state
        .db
        .get_agent_mcp_servers(&agent.id)
        .await
        .unwrap_or_default();

    let executor = AgentExecutor::new(
        Arc::clone(&state.db),
        agent,
        models,
        mcp_servers,
        state.event_tx.clone(),
    );

    tokio::spawn(async move {
        let _ = executor.run_turn(&payload.input).await;
    });

    Ok(Json(json!({ "status": "started" })))
}

pub async fn list_agent_pushed_messages_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let messages = state
        .db
        .list_pushed_messages(&id, 50)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(messages))
}

pub async fn get_agent_memory_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let mem = state
        .db
        .get_agent_memory(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({
        "agent_id": mem.agent_id,
        "content": mem.content,
        "current_hash": mem.hash,
        "character_count": mem.content.chars().count(),
        "updated_at": mem.updated_at
    })))
}

pub async fn get_agent_history_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let history = state
        .db
        .get_agent_history(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(history))
}

pub async fn clear_agent_history_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let cleared = state
        .db
        .clear_agent_history(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "cleared": cleared })))
}

pub async fn get_agent_tools_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let servers = state
        .db
        .get_agent_mcp_servers(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let permissions = state
        .db
        .get_agent_tool_permissions(&id)
        .await
        .unwrap_or_default();

    let subscriptions = state
        .db
        .get_agent_subscriptions(&id)
        .await
        .unwrap_or_default();

    let sub_set: std::collections::HashSet<String> = subscriptions.iter().cloned().collect();

    let mut tools_res = Vec::new();
    let mut resources_res = Vec::new();

    for s in servers {
        if s.transport == "wasm_stdio" || s.url_or_command.starts_with("wasm://") {
            let ext_id = s.id.strip_prefix("wasm:").unwrap_or(&s.id);
            if let Ok(Some(ext)) = state.db.get_extension(ext_id).await {
                if let Ok(manifest_val) = serde_json::from_str::<serde_json::Value>(&ext.manifest_json) {
                    if let Some(mcp) = manifest_val.get("mcp") {
                        if let Some(tool_list) = mcp.get("tools").and_then(|t| t.as_array()) {
                            for t in tool_list {
                                let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let desc = t.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let schema = t.get("inputSchema").cloned();
                                let is_enabled = permissions.iter().find(|p| p.mcp_id == s.id && p.tool_name == name).map(|p| p.is_enabled).unwrap_or(true);
                                let server_alias = s.get_alias();
                                let unique_name = format!("{}.{}", server_alias, name);

                                tools_res.push(AgentToolItem {
                                    mcp_id: s.id.clone(),
                                    server_name: s.name.clone(),
                                    server_alias,
                                    unique_name,
                                    name,
                                    description: desc,
                                    input_schema: schema,
                                    is_enabled,
                                });
                            }
                        }

                        if let Some(res_list) = mcp.get("resources").and_then(|r| r.as_array()) {
                            for r in res_list {
                                let uri = r.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or(&uri).to_string();
                                let desc = r.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let mime = r.get("mimeType").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let is_sub = sub_set.contains(&uri);

                                resources_res.push(AgentResourceItem {
                                    mcp_id: s.id.clone(),
                                    server_name: s.name.clone(),
                                    uri,
                                    name,
                                    description: desc,
                                    mime_type: mime,
                                    is_subscribed: is_sub,
                                });
                            }
                        }
                    }
                }
            }
        } else if s.transport == "stdio" {
            let (notif_tx, notif_rx) = tokio::sync::mpsc::unbounded_channel::<neuxdot_core::mcp::client::McpNotification>();
            if let Ok(transport) = neuxdot_core::mcp::stdio::StdioTransport::spawn(
                &s.url_or_command,
                s.headers_or_env.as_deref(),
                notif_tx,
            )
            .await
            {
                let session = neuxdot_core::mcp::client::McpClientSession {
                    server_id: s.id.clone(),
                    server_name: s.name.clone(),
                    transport,
                    notification_rx: tokio::sync::Mutex::new(notif_rx),
                };

                let fetch_fut = async {
                    let _ = session.initialize().await;
                    session.list_tools().await
                };

                if let Ok(Ok(tools)) = tokio::time::timeout(std::time::Duration::from_millis(3000), fetch_fut).await {
                    let server_alias = s.get_alias();
                    for tool in tools {
                        let is_enabled = permissions
                            .iter()
                            .find(|p| p.mcp_id == s.id && p.tool_name == tool.name)
                            .map(|p| p.is_enabled)
                            .unwrap_or(true);
                        let unique_name = format!("{}.{}", server_alias, tool.name);

                        tools_res.push(AgentToolItem {
                            mcp_id: s.id.clone(),
                            server_name: s.name.clone(),
                            server_alias: server_alias.clone(),
                            unique_name,
                            name: tool.name,
                            description: tool.description,
                            input_schema: tool.input_schema,
                            is_enabled,
                        });
                    }
                }

                let fetch_res_fut = async {
                    session.transport.send_request("resources/list", None).await
                };
                if let Ok(Ok(res_val)) = tokio::time::timeout(std::time::Duration::from_millis(1500), fetch_res_fut).await {
                    if let Some(res_list) = res_val.get("resources").and_then(|r| r.as_array()) {
                        for r in res_list {
                            let uri = r.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or(&uri).to_string();
                            let desc = r.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                            let mime = r.get("mimeType").and_then(|v| v.as_str()).map(|s| s.to_string());
                            let is_sub = sub_set.contains(&uri);

                            resources_res.push(AgentResourceItem {
                                mcp_id: s.id.clone(),
                                server_name: s.name.clone(),
                                uri,
                                name,
                                description: desc,
                                mime_type: mime,
                                is_subscribed: is_sub,
                            });
                        }
                    }
                }
            }
        } else {
            let (notif_tx, notif_rx) = tokio::sync::mpsc::unbounded_channel::<neuxdot_core::mcp::client::McpNotification>();
            let transport = neuxdot_core::mcp::http::HttpTransport::new(
                &s.url_or_command,
                s.headers_or_env.as_deref(),
                notif_tx,
            );
            let session = neuxdot_core::mcp::client::McpClientSession {
                server_id: s.id.clone(),
                server_name: s.name.clone(),
                transport,
                notification_rx: tokio::sync::Mutex::new(notif_rx),
            };

            let fetch_fut = async {
                let _ = session.initialize().await;
                session.list_tools().await
            };

            if let Ok(Ok(tools)) = tokio::time::timeout(std::time::Duration::from_millis(3500), fetch_fut).await {
                let server_alias = s.get_alias();
                for tool in tools {
                    let is_enabled = permissions
                        .iter()
                        .find(|p| p.mcp_id == s.id && p.tool_name == tool.name)
                        .map(|p| p.is_enabled)
                        .unwrap_or(true);
                    let unique_name = format!("{}.{}", server_alias, tool.name);

                    tools_res.push(AgentToolItem {
                        mcp_id: s.id.clone(),
                        server_name: s.name.clone(),
                        server_alias: server_alias.clone(),
                        unique_name,
                        name: tool.name,
                        description: tool.description,
                        input_schema: tool.input_schema,
                        is_enabled,
                    });
                }
            }

            let fetch_res_fut = async {
                session.transport.send_request("resources/list", None).await
            };
            if let Ok(Ok(res_val)) = tokio::time::timeout(std::time::Duration::from_millis(1500), fetch_res_fut).await {
                if let Some(res_list) = res_val.get("resources").and_then(|r| r.as_array()) {
                    for r in res_list {
                        let uri = r.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or(&uri).to_string();
                        let desc = r.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let mime = r.get("mimeType").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let is_sub = sub_set.contains(&uri);

                        resources_res.push(AgentResourceItem {
                            mcp_id: s.id.clone(),
                            server_name: s.name.clone(),
                            uri,
                            name,
                            description: desc,
                            mime_type: mime,
                            is_subscribed: is_sub,
                        });
                    }
                }
            }
        }
    }

    Ok(Json(json!({
        "tools": tools_res,
        "resources": resources_res,
        "subscriptions": subscriptions
    })))
}

pub async fn update_agent_tool_permission_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateToolPermissionRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    state
        .db
        .set_agent_tool_permission(&id, &payload.mcp_id, &payload.tool_name, payload.is_enabled)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "success": true })))
}

pub async fn get_agent_subscriptions_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let subscriptions = state
        .db
        .get_agent_subscriptions(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(subscriptions))
}

pub async fn update_agent_subscriptions_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateSubscriptionsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let agent = state
        .db
        .get_agent(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    if agent.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    state
        .db
        .set_agent_subscriptions(&id, &payload.subscriptions)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "success": true, "subscriptions": payload.subscriptions })))
}
