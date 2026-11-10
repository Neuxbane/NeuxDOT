use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use neuxdot_core::auth::{
    hash_password, validate_password_strength, verify_password, PASSWORD_MIN_LEN,
};
use neuxdot_core::db::schema::{is_valid_role, Account, Profile, ROLE_ROOT};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

type ApiError = (StatusCode, Json<serde_json::Value>);

fn internal_error(err: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": err.to_string() })),
    )
}

fn bad_request(message: impl Into<String>) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}

#[derive(Debug, Serialize)]
pub struct AccountSummary {
    pub id: String,
    pub username: String,
    pub role: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

fn to_summary(account: Account, profile: Option<Profile>) -> AccountSummary {
    let profile = profile.unwrap_or(Profile {
        account_id: account.id.clone(),
        display_name: None,
        avatar_color: None,
        avatar_shape: None,
        avatar_url: None,
    });
    AccountSummary {
        id: account.id,
        username: account.username,
        role: account.role,
        display_name: profile.display_name.filter(|name| !name.trim().is_empty()),
        avatar_url: profile.avatar_url.filter(|url| !url.trim().is_empty()),
        created_at: account.created_at,
    }
}

/// `GET /api/accounts` — list all accounts (root only).
pub async fn list_accounts_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    auth.require_root()?;

    let accounts = state.db.list_accounts().await.map_err(internal_error)?;
    let mut result = Vec::with_capacity(accounts.len());
    for account in accounts {
        let profile = state
            .db
            .get_profile(&account.id)
            .await
            .map_err(internal_error)?;
        result.push(to_summary(account, profile));
    }

    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateAccountRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub avatar_url: Option<String>,
}

/// `POST /api/accounts` — create an account (root only).
pub async fn create_account_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateAccountRequest>,
) -> Result<impl IntoResponse, ApiError> {
    auth.require_root()?;

    let username = payload.username.trim().to_string();
    if username.is_empty() {
        return Err(bad_request("Username must not be empty"));
    }

    let role = payload.role.unwrap_or_else(|| "member".to_string());
    if !is_valid_role(&role) {
        return Err(bad_request(format!(
            "Invalid role '{}': expected 'root' or 'member'",
            role
        )));
    }

    if let Err(err) = validate_password_strength(&payload.password) {
        return Err(bad_request(format!("Weak password: {}", err)));
    }

    if state
        .db
        .get_account_by_username(&username)
        .await
        .map_err(internal_error)?
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "Username already exists" })),
        ));
    }

    let password_hash = hash_password(&payload.password)
        .map_err(|e| internal_error(format!("Password hashing error: {}", e)))?;

    let account = state
        .db
        .create_account_with_role(&username, &password_hash, &role)
        .await
        .map_err(internal_error)?;

    let display_name = payload
        .display_name
        .clone()
        .unwrap_or_else(|| username.clone());
    let profile = state
        .db
        .update_profile(
            &account.id,
            Some(display_name.as_str()),
            None,
            None,
            payload.avatar_url.as_deref(),
        )
        .await
        .map_err(internal_error)?;

    Ok((
        StatusCode::CREATED,
        Json(to_summary(account, Some(profile))),
    ))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAccountRequest {
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub avatar_url: Option<String>,
}

/// `PATCH /api/accounts/:id` — edit username/display name/role/avatar (root only).
///
/// The last remaining root account cannot have its role changed away from `root`.
pub async fn update_account_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(payload): Json<UpdateAccountRequest>,
) -> Result<impl IntoResponse, ApiError> {
    auth.require_root()?;

    let account = state
        .db
        .get_account_by_id(&account_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Account not found" })),
            )
        })?;

    let username = match payload.username {
        Some(ref raw) => {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                return Err(bad_request("Username must not be empty"));
            }
            if trimmed != account.username {
                if state
                    .db
                    .get_account_by_username(&trimmed)
                    .await
                    .map_err(internal_error)?
                    .is_some()
                {
                    return Err((
                        StatusCode::CONFLICT,
                        Json(json!({ "error": "Username already exists" })),
                    ));
                }
            }
            Some(trimmed)
        }
        None => None,
    };

    let role = match payload.role {
        Some(ref new_role) => {
            if !is_valid_role(new_role) {
                return Err(bad_request(format!(
                    "Invalid role '{}': expected 'root' or 'member'",
                    new_role
                )));
            }
            // Demoting a root is only allowed while another root remains.
            if account.is_root()
                && new_role.as_str() != ROLE_ROOT
                && state
                    .db
                    .is_last_root(&account.id)
                    .await
                    .map_err(internal_error)?
            {
                return Err(bad_request("Cannot demote the last remaining root account"));
            }
            Some(new_role.clone())
        }
        None => None,
    };

    let updated_account = state
        .db
        .update_account(&account.id, username.as_deref(), role.as_deref())
        .await
        .map_err(internal_error)?;

    let profile = state
        .db
        .update_profile(
            &account.id,
            payload.display_name.as_deref(),
            None,
            None,
            payload.avatar_url.as_deref(),
        )
        .await
        .map_err(internal_error)?;

    Ok(Json(to_summary(updated_account, Some(profile))))
}

/// `DELETE /api/accounts/:id` — delete an account (root only).
///
/// Refuses to delete the calling root account (`403`) or the last remaining root (`400`).
pub async fn delete_account_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(account_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    auth.require_root()?;

    if account_id == auth.account_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "You cannot delete your own account" })),
        ));
    }

    let account = state
        .db
        .get_account_by_id(&account_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Account not found" })),
            )
        })?;

    if account.is_root()
        && state
            .db
            .is_last_root(&account.id)
            .await
            .map_err(internal_error)?
    {
        return Err(bad_request("Cannot delete the last remaining root account"));
    }

    let deleted = state
        .db
        .delete_account(&account_id)
        .await
        .map_err(internal_error)?;
    if !deleted {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Account not found" })),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: Option<String>,
    pub new_password: String,
    pub confirm_password: String,
}

/// `POST /api/accounts/:id/password` — change an account password.
///
/// Allowed for the account itself (old password required, `401` when wrong) or for
/// any `root` account (old password optional). `old_password` is mandatory for
/// non-root callers changing somebody else's password (which they cannot do anyway).
pub async fn change_password_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let is_self = account_id == auth.account_id;
    if !is_self && !auth.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Forbidden: root role required" })),
        ));
    }

    let account = state
        .db
        .get_account_by_id(&account_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Account not found" })),
            )
        })?;

    let old_password = payload
        .old_password
        .as_deref()
        .filter(|value| !value.is_empty());

    // Self-service changes always require the current password. Root may reset
    // another account without it, but if one is supplied it must be correct.
    if is_self && old_password.is_none() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Current password is required" })),
        ));
    }
    if let Some(old_password) = old_password {
        if !verify_password(old_password, &account.password_hash) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Current password is incorrect" })),
            ));
        }
    }

    if payload.new_password != payload.confirm_password {
        return Err(bad_request("New password and confirmation do not match"));
    }

    if let Err(err) = validate_password_strength(&payload.new_password) {
        return Err(bad_request(format!(
            "Weak password (minimum {} characters, letters and digits): {}",
            PASSWORD_MIN_LEN, err
        )));
    }

    let new_hash = hash_password(&payload.new_password)
        .map_err(|e| internal_error(format!("Password hashing error: {}", e)))?;
    state
        .db
        .update_account_password(&account.id, &new_hash)
        .await
        .map_err(internal_error)?;

    Ok(Json(json!({ "ok": true, "account_id": account.id })))
}

#[derive(Deserialize)]
pub struct SearchAccountsQuery {
    #[serde(default)]
    pub q: String,
}

/// `GET /api/accounts/search?q=...`
///
/// Deliberately **not** root-only: any signed-in user needs to find someone to
/// share a bot with. Only non-sensitive fields are returned, so this cannot be
/// used to enumerate password hashes, roles or creation dates.
pub async fn search_accounts_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<SearchAccountsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let rows = state
        .db
        .search_accounts_for_sharing(&query.q, 20)
        .await
        .map_err(internal_error)?;

    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(account_id, username, display_name, avatar_url)| {
            json!({
                "account_id": account_id,
                "username": username,
                "display_name": display_name,
                "avatar_url": avatar_url,
            })
        })
        .collect();

    Ok(Json(items))
}
