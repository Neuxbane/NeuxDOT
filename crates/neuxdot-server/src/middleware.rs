use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use neuxdot_core::auth::verify_token;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub account_id: String,
    pub username: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // 1. Try Authorization header
        if let Some(auth_val) = parts.headers.get(header::AUTHORIZATION) {
            if let Ok(auth_str) = auth_val.to_str() {
                if let Some(token) = auth_str.strip_prefix("Bearer ") {
                    if let Ok(claims) = verify_token(token) {
                        return Ok(AuthUser {
                            account_id: claims.sub,
                            username: claims.username,
                        });
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
                                return Ok(AuthUser {
                                    account_id: claims.sub,
                                    username: claims.username,
                                });
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
