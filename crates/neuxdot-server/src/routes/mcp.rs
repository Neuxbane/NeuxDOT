use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use neuxdot_core::events::{AgentEvent, EventEnvelope};
use serde::Deserialize;
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateMcpRequest {
    pub name: String,
    pub transport: String, // "stdio" or "http"
    pub url_or_command: String,
}

#[derive(Deserialize)]
pub struct UpdateMcpRequest {
    pub name: String,
    pub transport: String,
    pub url_or_command: String,
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
    if transport != "stdio" && transport != "http" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Transport must be 'stdio' or 'http'" })),
        ));
    }

    let server = state
        .db
        .create_mcp_server(
            &auth.account_id,
            &payload.name,
            &transport,
            &payload.url_or_command,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok((StatusCode::CREATED, Json(server)))
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
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "MCP server not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let transport = payload.transport.to_lowercase();
    if transport != "stdio" && transport != "http" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Transport must be 'stdio' or 'http'" })),
        ));
    }

    let updated = state
        .db
        .update_mcp_server(&id, &payload.name, &transport, &payload.url_or_command)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(updated))
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
