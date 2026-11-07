//! Thin HTTP layer over the GitLab REST and GraphQL APIs.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};

use crate::config::{AuthKind, Config};

tokio::task_local! {
    pub static CURRENT_GITLAB_TOKEN: Option<String>;
}

/// A GitLab response reduced to what the tool layer needs.
pub struct ApiResponse {
    #[allow(dead_code)]
    pub status: StatusCode,
    pub body: Value,
    /// Pagination headers, present on list endpoints.
    pub next_page: Option<String>,
    pub total: Option<String>,
    pub next_page_token: Option<String>,
}

/// An error carrying GitLab's own message, so the model sees why a call failed.
#[derive(Debug, thiserror::Error)]
#[error("GitLab {status}: {message}")]
pub struct ApiError {
    #[allow(dead_code)]
    pub status: StatusCode,
    pub message: String,
}

pub struct GitLabClient {
    http: Client,
    api_url: String,
    graphql_url: String,
    pub default_token: Option<String>,
    pub default_auth: AuthKind,
    max_response_bytes: usize,
}

impl GitLabClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&format!("gitlab-mcp-rs/{}", env!("CARGO_PKG_VERSION")))?,
        );

        let http = Client::builder()
            .default_headers(headers)
            .timeout(cfg.timeout)
            .connect_timeout(Duration::from_secs(15))
            .danger_accept_invalid_certs(cfg.insecure_tls)
            .build()
            .context("building the HTTP client")?;

        Ok(Self {
            http,
            api_url: cfg.api_url.clone(),
            graphql_url: cfg.graphql_url.clone(),
            default_token: cfg.token.clone(),
            default_auth: cfg.auth,
            max_response_bytes: cfg.max_response_bytes,
        })
    }

    fn apply_auth(&self, mut req: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
        let tok = token.or(self.default_token.as_deref());
        if let Some(t) = tok {
            let t = t.trim();
            if !t.is_empty() {
                let is_bearer = t.starts_with("gloas-")
                    || t.starts_with("Bearer ")
                    || self.default_auth == AuthKind::Bearer;
                if is_bearer {
                    let bearer = if t.starts_with("Bearer ") {
                        t.to_string()
                    } else {
                        format!("Bearer {t}")
                    };
                    if let Ok(mut h) = HeaderValue::from_str(&bearer) {
                        h.set_sensitive(true);
                        req = req.header("Authorization", h);
                    }
                } else if let Ok(mut h) = HeaderValue::from_str(t) {
                    h.set_sensitive(true);
                    req = req.header("PRIVATE-TOKEN", h);
                }
            }
        }
        req
    }

    /// Issue a REST call using default token if configured.
    pub async fn rest(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<ApiResponse, ApiError> {
        self.rest_with_auth(method, path, query, body, None).await
    }

    /// Issue a REST call with dynamic per-call token support.
    pub async fn rest_with_auth(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
        token: Option<&str>,
    ) -> Result<ApiResponse, ApiError> {
        let url = format!("{}{}", self.api_url, path);
        let mut req = self.http.request(method, &url);
        req = self.apply_auth(req, token);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| ApiError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("request to {url} failed: {e}"),
        })?;
        self.finish(resp).await
    }

    /// Issue a GraphQL call using default token.
    pub async fn graphql(&self, query: &str, variables: Value) -> Result<ApiResponse, ApiError> {
        self.graphql_with_auth(query, variables, None).await
    }

    /// Issue a GraphQL call with dynamic per-call token support.
    pub async fn graphql_with_auth(
        &self,
        query: &str,
        variables: Value,
        token: Option<&str>,
    ) -> Result<ApiResponse, ApiError> {
        let req = self
            .http
            .post(&self.graphql_url)
            .json(&json!({ "query": query, "variables": variables }));
        let req = self.apply_auth(req, token);
        let resp = req
            .send()
            .await
            .map_err(|e| ApiError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("GraphQL request failed: {e}"),
            })?;
        let out = self.finish(resp).await?;
        // GraphQL reports failures with HTTP 200 and an `errors` array.
        if let Some(errors) = out.body.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let msgs: Vec<&str> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect();
                return Err(ApiError {
                    status: StatusCode::BAD_REQUEST,
                    message: msgs.join("; "),
                });
            }
        }
        Ok(out)
    }

    async fn finish(&self, resp: reqwest::Response) -> Result<ApiResponse, ApiError> {
        let status = resp.status();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .filter(|s| !s.is_empty())
        };
        let next_page = header("x-next-page");
        let total = header("x-total");
        let next_page_token = header("x-next-page-token");

        let text = resp.text().await.map_err(|e| ApiError {
            status,
            message: format!("could not read the response body: {e}"),
        })?;

        if !status.is_success() {
            if status == StatusCode::UNAUTHORIZED {
                let has_token = CURRENT_GITLAB_TOKEN.try_with(|t| t.is_some()).unwrap_or(false)
                    || self.default_token.is_some();
                if !has_token {
                    return Err(ApiError {
                        status,
                        message: "401 Unauthorized: No GitLab token provided. Pass 'gitlab_token' or 'token' in tool arguments, or set GITLAB_PERSONAL_ACCESS_TOKEN".to_string(),
                    });
                }
            }
            return Err(ApiError {
                status,
                message: extract_error_message(&text, status),
            });
        }

        // 204 and other empty successes have no JSON body.
        let body = if text.trim().is_empty() {
            json!({ "success": true, "status": status.as_u16() })
        } else {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => v,
                Err(_) => Value::String(truncate(text, self.max_response_bytes)),
            }
        };

        Ok(ApiResponse { status, body, next_page, total, next_page_token })
    }
}

/// GitLab reports errors as `{"message": ...}` or `{"error": ...}`, sometimes as
/// a plain string, sometimes as a map of field errors.
fn extract_error_message(text: &str, status: StatusCode) -> String {
    let fallback = || {
        if text.trim().is_empty() {
            status.canonical_reason().unwrap_or("request failed").to_string()
        } else {
            truncate(text.to_string(), 2000)
        }
    };
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return fallback();
    };
    for key in ["message", "error", "error_description"] {
        match v.get(key) {
            Some(Value::String(s)) => return s.clone(),
            Some(other @ (Value::Object(_) | Value::Array(_))) => return other.to_string(),
            _ => {}
        }
    }
    fallback()
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("\n… [truncated]");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gitlab_message_field() {
        let m = extract_error_message(r#"{"message":"404 Project Not Found"}"#, StatusCode::NOT_FOUND);
        assert_eq!(m, "404 Project Not Found");
    }

    #[test]
    fn reads_gitlab_error_field() {
        let m = extract_error_message(r#"{"error":"insufficient_scope"}"#, StatusCode::FORBIDDEN);
        assert_eq!(m, "insufficient_scope");
    }

    #[test]
    fn keeps_field_error_maps_intact() {
        let m = extract_error_message(r#"{"message":{"title":["can't be blank"]}}"#, StatusCode::BAD_REQUEST);
        assert!(m.contains("can't be blank"));
    }

    #[test]
    fn falls_back_to_status_reason_on_empty_body() {
        let m = extract_error_message("", StatusCode::UNAUTHORIZED);
        assert_eq!(m, "Unauthorized");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "привет мир".repeat(50);
        let t = truncate(s, 21);
        assert!(t.ends_with("[truncated]"));
    }
}
