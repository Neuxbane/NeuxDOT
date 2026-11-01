use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use neuxdot_core::auth::{create_token, verify_password};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub account_id: String,
    pub username: String,
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let account = state
        .db
        .get_account_by_username(&payload.username)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid username or password" })),
            )
        })?;

    if !verify_password(&payload.password, &account.password_hash) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid username or password" })),
        ));
    }

    let token = create_token(&account.id, &account.username).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    let mut headers = HeaderMap::new();
    let cookie_val = format!("token={}; Path=/; HttpOnly; SameSite=Lax", token);
    if let Ok(hv) = HeaderValue::from_str(&cookie_val) {
        headers.insert(header::SET_COOKIE, hv);
    }

    Ok((
        headers,
        Json(AuthResponse {
            token,
            account_id: account.id,
            username: account.username,
        }),
    ))
}

pub async fn me_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let profile = state
        .db
        .get_profile(&auth.account_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(json!({
        "account_id": auth.account_id,
        "username": auth.username,
        "profile": profile
    })))
}
