//! Per-account "Agent Instructions" presets.
//!
//! Every handler is owner-scoped: a preset that belongs to another account is
//! reported as `404 Not Found` rather than `403`, so the existence of other
//! users' presets is never leaked.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

type ApiError = (StatusCode, Json<serde_json::Value>);

fn internal_error(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() })))
}

fn not_found() -> ApiError {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "Instruction preset not found" })))
}

#[derive(Deserialize)]
pub struct CreateInstructionPresetRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub content: String,
}

#[derive(Deserialize)]
pub struct UpdateInstructionPresetRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
}

/// `GET /api/instruction-presets`
pub async fn list_instruction_presets_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let presets = state
        .db
        .list_instruction_presets(&auth.account_id)
        .await
        .map_err(internal_error)?;
    Ok(Json(presets))
}

/// `POST /api/instruction-presets`
pub async fn create_instruction_preset_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateInstructionPresetRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = payload.name.trim();
    if name.is_empty() {
        return Err(bad_request("Name must not be empty"));
    }
    if payload.content.trim().is_empty() {
        return Err(bad_request("Content must not be empty"));
    }

    let created = state
        .db
        .create_instruction_preset(
            &auth.account_id,
            name,
            payload.description.as_deref().unwrap_or("").trim(),
            &payload.content,
        )
        .await
        .map_err(internal_error)?;

    Ok((StatusCode::CREATED, Json(created)))
}

/// `PATCH /api/instruction-presets/:id`
pub async fn update_instruction_preset_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateInstructionPresetRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(ref name) = payload.name {
        if name.trim().is_empty() {
            return Err(bad_request("Name must not be empty"));
        }
    }
    if let Some(ref content) = payload.content {
        if content.trim().is_empty() {
            return Err(bad_request("Content must not be empty"));
        }
    }

    let updated = state
        .db
        .update_instruction_preset(
            &auth.account_id,
            &id,
            payload.name.as_deref().map(str::trim),
            payload.description.as_deref().map(str::trim),
            payload.content.as_deref(),
        )
        .await
        .map_err(internal_error)?
        .ok_or_else(not_found)?;

    Ok(Json(updated))
}

/// `DELETE /api/instruction-presets/:id`
pub async fn delete_instruction_preset_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let deleted = state
        .db
        .delete_instruction_preset(&auth.account_id, &id)
        .await
        .map_err(internal_error)?;

    if !deleted {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
