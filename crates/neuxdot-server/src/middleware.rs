use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use axum::extract::FromRef;
use neuxdot_core::auth::verify_token;
use neuxdot_core::db::schema::{is_valid_role, ROLE_MEMBER, ROLE_ROOT};

use crate::state::AppState;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub account_id: String,
    pub username: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_root(&self) -> bool {
        self.role == ROLE_ROOT
    }

    /// Returns `403 Forbidden` unless the caller holds the `root` role.
    ///
    /// The error type matches the `(StatusCode, Json<Value>)` shape used by every
    /// route handler, so handlers can simply `auth.require_root()?`.
    pub fn require_root(
        &self,
    ) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
        if self.is_root() {
            Ok(())
        } else {
            Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Forbidden: root role required" })),
            ))
        }
    }
}

/// Resolves the caller's current identity from the database.
///
/// The role in the JWT is only a hint: tokens live for 30 days, so a demoted or
/// deleted account would otherwise keep its privileges until expiry. Authority
/// is therefore read from `accounts` on every request; a token whose account no
/// longer exists is rejected outright.
async fn resolve_identity(
    state: &AppState,
    claims: neuxdot_core::auth::Claims,
) -> Result<AuthUser, Response> {
    let account = match state.db.get_account_by_id(&claims.sub).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            let body = Json(json!({ "error": "Unauthorized: account no longer exists" }));
            return Err((StatusCode::UNAUTHORIZED, body).into_response());
        }
        Err(e) => {
            let body = Json(json!({ "error": format!("Identity lookup failed: {}", e) }));
            return Err((StatusCode::INTERNAL_SERVER_ERROR, body).into_response());
        }
    };

    let role = if is_valid_role(&account.role) {
        account.role
    } else {
        ROLE_MEMBER.to_string()
    };

    Ok(AuthUser {
        account_id: account.id,
        username: account.username,
        role,
    })
}

#[allow(dead_code)]
fn auth_user_from_claims(claims: neuxdot_core::auth::Claims) -> AuthUser {
    AuthUser {
        account_id: claims.sub,
        username: claims.username,
        role: claims.role,
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        // 1. Try Authorization header
        if let Some(auth_val) = parts.headers.get(header::AUTHORIZATION) {
            if let Ok(auth_str) = auth_val.to_str() {
                if let Some(token) = auth_str.strip_prefix("Bearer ") {
                    if let Ok(claims) = verify_token(token) {
                        return resolve_identity(&app_state, claims).await;
                    }
                }
            }
        }

        // 2. Try Cookie header
        if let Some(cookie_val) = parts.headers.get(header::COOKIE) {
            if let Ok(cookie_str) = cookie_val.to_str() {
                for pair in cookie_str.split(';') {
                    let mut parts_kv = pair.trim().splitn(2, '=');
                    if let (Some(k), Some(v)) = (parts_kv.next(), parts_kv.next()) {
                        if k == "token" {
                            if let Ok(claims) = verify_token(v) {
                                return resolve_identity(&app_state, claims).await;
                            }
                        }
                    }
                }
            }
        }

        let body = Json(json!({ "error": "Unauthorized" }));
        Err((StatusCode::UNAUTHORIZED, body).into_response())
    }
}
