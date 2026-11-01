use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
}

pub async fn get_profile_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let profile = state
        .db
        .get_profile(&auth.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(profile))
}

pub async fn update_profile_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let updated = state
        .db
        .update_profile(
            &auth.account_id,
            payload.display_name.as_deref(),
            payload.avatar_color.as_deref(),
            payload.avatar_shape.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(updated))
}
