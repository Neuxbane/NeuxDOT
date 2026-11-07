use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use neuxdot_core::db::schema::{Agent, AgentAccess};
use neuxdot_core::runtime::executor::AgentExecutor;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::middleware::AuthUser;
use crate::state::AppState;

type ApiError = (StatusCode, Json<serde_json::Value>);

fn internal_error(e: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
}

/// Loads an agent and resolves the caller's rights over it.
///
/// `404` when the agent does not exist, `403` when the caller has no access at
/// all. Handlers then gate on the returned permission flags.
async fn load_agent_with_access(
    state: &AppState,
    agent_id: &str,
    account_id: &str,
) -> Result<(Agent, AgentAccess), ApiError> {
    let agent = state
        .db
        .get_agent(agent_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Agent not found" }))))?;

    let access = state
        .db
        .resolve_agent_access(&agent, account_id)
        .await
        .map_err(internal_error)?;

    if !access.can_view() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied" })),
        ));
    }
    Ok((agent, access))
}

fn require_edit(access: AgentAccess) -> Result<(), ApiError> {
    if access.can_edit() {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "You do not have permission to edit this agent" })),
        ))
    }
}

fn require_manage_shares(access: AgentAccess) -> Result<(), ApiError> {
    if access.can_manage_shares() {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "You do not have permission to manage sharing for this agent" })),
        ))
    }
}

#[derive(Serialize)]
pub struct AgentToolItem {
    pub mcp_id: String,
    pub server_name: String,
    pub server_alias: String,
    pub name: String,
    pub unique_name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
    pub meta: Option<serde_json::Value>,
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
    let rows = state
        .db
        .list_agents_for_account(&auth.account_id)
        .await
        .map_err(internal_error)?;

    let agents: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(agent, is_owner, can_edit, can_manage_shares)| {
            let mut value = serde_json::to_value(&agent).unwrap_or_else(|_| json!({}));
            value["is_owner"] = json!(is_owner);
            value["can_edit"] = json!(can_edit);
            value["can_manage_shares"] = json!(can_manage_shares);
            value
        })
        .collect();

    Ok(Json(agents))
}

pub async fn get_agent_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
	let (agent, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
        // Caller's rights over this agent, so the UI can hide what it may not do.
        "is_owner": access.is_owner(),
        "can_edit": access.can_edit(),
        "can_manage_shares": access.can_manage_shares(),
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
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_edit(access)?;

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

    // Deleting is owner-only: a sharee can leave, but never destroy the bot.
    if existing.account_id != auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Only the owner can delete this agent" })),
        ));
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
	let (agent, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
    let (_, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
    let (_, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
    let (_, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_edit(access)?;

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
    let (_, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
        if !s.is_active {
            continue;
        }
        if s.transport == "stdio" || s.transport == "binary" {
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
                        if tool.is_app_only() {
                            continue;
                        }
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
                            meta: tool.meta,
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
                    if tool.is_app_only() {
                        continue;
                    }
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
                        meta: tool.meta,
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
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_edit(access)?;

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
    let (_, _access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

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
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_edit(access)?;

    state
        .db
        .set_agent_subscriptions(&id, &payload.subscriptions)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "success": true, "subscriptions": payload.subscriptions })))
}

// --- Agent sharing ---

#[derive(Deserialize)]
pub struct CreateAgentShareRequest {
    pub username: String,
    #[serde(default)]
    pub can_edit: bool,
    #[serde(default)]
    pub can_manage_shares: bool,
}

#[derive(Deserialize)]
pub struct UpdateAgentShareRequest {
    pub can_edit: Option<bool>,
    pub can_manage_shares: Option<bool>,
}

/// `GET /api/agents/:id/shares`
pub async fn list_agent_shares_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_manage_shares(access)?;

    let shares = state.db.list_agent_shares(&id).await.map_err(internal_error)?;
    Ok(Json(shares))
}

/// `POST /api/agents/:id/shares`
pub async fn create_agent_share_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<CreateAgentShareRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (agent, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_manage_shares(access)?;

    let username = payload.username.trim();
    if username.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "username must not be empty" })),
        ));
    }

    let Some(account) = state
        .db
        .get_account_by_username(username)
        .await
        .map_err(internal_error)?
    else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("No account named '{}'", username) })),
        ));
    };

    if account.id == agent.account_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "The owner already has full access" })),
        ));
    }

    if state
        .db
        .get_agent_share(&id, &account.id)
        .await
        .map_err(internal_error)?
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "This agent is already shared with that account" })),
        ));
    }

    let share = state
        .db
        .upsert_agent_share(&id, &account.id, payload.can_edit, payload.can_manage_shares)
        .await
        .map_err(internal_error)?;

    Ok((StatusCode::CREATED, Json(json!(share))))
}

/// `PATCH /api/agents/:id/shares/:account_id`
pub async fn update_agent_share_handler(
    auth: AuthUser,
    Path((id, account_id)): Path<(String, String)>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateAgentShareRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (_, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
    require_manage_shares(access)?;

    let updated = state
        .db
        .update_agent_share(&id, &account_id, payload.can_edit, payload.can_manage_shares)
        .await
        .map_err(internal_error)?;

    updated
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Share not found" }))))
}

/// `DELETE /api/agents/:id/shares/:account_id`
///
/// Requires share-management rights, **except** that any account may always
/// remove its own access (leave the bot).
pub async fn delete_agent_share_handler(
    auth: AuthUser,
    Path((id, account_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let (agent, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;

    // The owner is never a "share", so removing them is meaningless.
    if account_id == agent.account_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "The owner always retains access" })),
        ));
    }

    let is_self = account_id == auth.account_id;
    if !is_self {
        require_manage_shares(access)?;
    }

    let removed = state
        .db
        .delete_agent_share(&id, &account_id)
        .await
        .map_err(internal_error)?;

    if !removed {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Share not found" })),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}
