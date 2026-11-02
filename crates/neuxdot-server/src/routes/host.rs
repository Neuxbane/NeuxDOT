//! Host bridge endpoints for sandboxed MCP-Apps.
//!
//! A hosted UI runs in an `<iframe srcdoc sandbox="allow-scripts">`, i.e. an
//! **opaque origin**. It cannot read the profile, cannot resolve relative URLs
//! and must never be handed a raw `file:`/`data:` link. Everything it is allowed
//! to know is therefore resolved *here*, server-side, against the authenticated
//! session — this module is the "API proxy" half of `docs/HOST_BRIDGE.md`.
//!
//! Consent is authoritative on this side: every value is emitted only when the
//! corresponding grant is present, and anything ungranted is **omitted** rather
//! than blanked (contract §3, "omission rule").

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use neuxdot_core::db::schema::{Agent, McpServer, PluginPermissions};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

/// Protocol banner echoed into the bootstrap.
pub const HOST_BRIDGE_PROTOCOL_VERSION: &str = "2025-03-26";

// ---------------------------------------------------------------------------
// Response shapes (kept structurally identical to `HostBridgeBootstrap` in
// apps/web/src/lib/api.ts).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AvatarRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AgentIdentity {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<AvatarRef>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UserIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<AvatarRef>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AppDescriptor {
    #[serde(rename = "pluginId")]
    pub plugin_id: String,
    #[serde(rename = "pluginName")]
    pub plugin_name: String,
    #[serde(rename = "resourceUri")]
    pub resource_uri: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BridgeContext {
    pub locale: Option<String>,
    pub theme: String,
}

/// The payload pushed on `ui/context` and returned by `ui/initialize`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HostBridgeBootstrap {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub app: AppDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserIdentity>,
    pub capabilities: Vec<String>,
    pub permissions: PluginPermissions,
    pub context: BridgeContext,
}

/// Build the `agent` block, honouring the two agent identity grants.
///
/// Returns `None` when neither grant is present, so the key is omitted
/// entirely instead of shipping a half-empty object.
pub fn build_agent_identity(agent: &Agent, perms: &PluginPermissions) -> Option<AgentIdentity> {
    let name = if perms.identity.agent_name {
        Some(agent.name.clone())
    } else {
        None
    };
    let avatar = if perms.identity.agent_profile_picture {
        Some(AvatarRef {
            url: agent.avatar_url.clone(),
            color: agent.avatar_color.clone(),
            shape: agent.avatar_shape.clone(),
        })
    } else {
        None
    };
    if name.is_none() && avatar.is_none() {
        return None;
    }
    Some(AgentIdentity {
        id: agent.id.clone(),
        name,
        avatar,
    })
}

/// Build the `user` block from the signed-in account's profile row.
pub fn build_user_identity(
    profile: Option<&neuxdot_core::db::schema::Profile>,
    fallback_username: &str,
    perms: &PluginPermissions,
) -> Option<UserIdentity> {
    let name = if perms.identity.user_name {
        Some(
            profile
                .and_then(|p| p.display_name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| fallback_username.to_string()),
        )
    } else {
        None
    };
    let avatar = if perms.identity.user_profile_picture {
        Some(AvatarRef {
            url: profile.and_then(|p| p.avatar_url.clone()),
            color: profile.and_then(|p| p.avatar_color.clone()),
            shape: profile.and_then(|p| p.avatar_shape.clone()),
        })
    } else {
        None
    };
    if name.is_none() && avatar.is_none() {
        return None;
    }
    Some(UserIdentity { name, avatar })
}

/// Assemble the whole bootstrap from already-loaded rows.
pub fn build_bootstrap(
    server: &McpServer,
    agent: &Agent,
    profile: Option<&neuxdot_core::db::schema::Profile>,
    username: &str,
    resource_uri: &str,
    theme: &str,
) -> HostBridgeBootstrap {
    let perms = server.get_permissions();
    let title = if server.name.trim().is_empty() {
        server.get_alias()
    } else {
        server.name.clone()
    };

    HostBridgeBootstrap {
        protocol_version: HOST_BRIDGE_PROTOCOL_VERSION.to_string(),
        app: AppDescriptor {
            plugin_id: server.id.clone(),
            plugin_name: server.name.clone(),
            resource_uri: resource_uri.to_string(),
            title,
        },
        agent: build_agent_identity(agent, &perms),
        user: build_user_identity(profile, username, &perms),
        capabilities: perms.capabilities(),
        permissions: perms,
        context: BridgeContext {
            locale: None,
            theme: theme.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Storage key scoping
// ---------------------------------------------------------------------------

/// Storage namespace for a plugin, isolated per account **and** plugin, so one
/// plugin can never read another plugin's (or another account's) keys.
///
/// Reuses the existing `extension_kv` table instead of adding a parallel store.
pub fn storage_namespace(account_id: &str, plugin_id: &str) -> String {
    let plugin = plugin_id.strip_prefix("wasm:").unwrap_or(plugin_id);
    format!("host:{}:{}", account_id, plugin)
}

/// Validate a storage operation against the resolved grants.
///
/// Errors are `(status, message)` so callers can map them straight onto HTTP.
pub fn check_storage_access(
    perms: &PluginPermissions,
    verb: &str,
    key: &str,
) -> Result<(), (StatusCode, String)> {
    if key.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "key must not be empty".to_string()));
    }
    match perms.check_storage(verb, key) {
        Ok(()) => Ok(()),
        Err(reason) => {
            // Distinguish "not granted" from "malformed" for the client.
            let code = if reason.contains("not granted") || reason.contains("outside the allowed") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((code, reason))
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    pub plugin_id: String,
    pub agent_id: String,
    #[serde(default)]
    pub resource_uri: String,
    #[serde(default)]
    pub theme: Option<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn api_err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(json!({ "error": msg.into() })))
}

/// Load a plugin, enforcing that it belongs to the caller.
async fn load_owned_server(state: &AppState, account_id: &str, id: &str) -> Result<McpServer, ApiError> {
    let server = state
        .db
        .get_mcp_server(id)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .or_else(|| None);
    // Some rows are stored with the `wasm:` prefix stripped.
    let server = match server {
        Some(s) => s,
        None => state
            .db
            .get_mcp_server(&format!("wasm:{}", id))
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Plugin not found"))?,
    };
    if server.account_id != account_id {
        return Err(api_err(StatusCode::FORBIDDEN, "Access denied"));
    }
    Ok(server)
}

/// `GET /api/host/session` — server-resolved bridge bootstrap.
pub async fn host_session_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<SessionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let server = load_owned_server(&state, &auth.account_id, &q.plugin_id).await?;

    let agent = state
        .db
        .get_agent(&q.agent_id)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Agent not found"))?;
    if agent.account_id != auth.account_id {
        return Err(api_err(StatusCode::FORBIDDEN, "Access denied"));
    }

    let profile = state
        .db
        .get_profile(&auth.account_id)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let theme = match q.theme.as_deref() {
        Some("dark") => "dark",
        _ => "light",
    };

    let bootstrap = build_bootstrap(
        &server,
        &agent,
        profile.as_ref(),
        &auth.username,
        &q.resource_uri,
        theme,
    );
    Ok(Json(bootstrap))
}

#[derive(Debug, Deserialize)]
pub struct AvatarQuery {
    pub kind: String,
    pub id: String,
}

/// Decode a `data:` URL into `(content_type, bytes)`.
fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let is_base64 = meta.ends_with(";base64");
    let mime = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = if is_base64 {
        decode_base64(payload)?
    } else {
        payload.as_bytes().to_vec()
    };
    Some((mime, bytes))
}

/// Minimal standard-alphabet base64 decoder (padding optional).
///
/// Hand-rolled to avoid pulling another crate into the server for one call
/// site; avatars are the only base64 we ever decode here.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in input.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' || c == b' ' || c == b'\t' {
            continue;
        }
        let v = value(c)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// `GET /api/host/avatar` — stream the stored avatar for the sandboxed app.
///
/// Serving the bytes from the API avoids handing the opaque-origin iframe a raw
/// `data:` URL (often megabytes) or any scheme it cannot load.
pub async fn host_avatar_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<AvatarQuery>,
) -> Result<Response, ApiError> {
    let avatar_url = match q.kind.as_str() {
        "user" => {
            // Always the caller's own profile; a foreign id is simply their own.
            let profile = state
                .db
                .get_profile(&auth.account_id)
                .await
                .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            profile.and_then(|p| p.avatar_url)
        }
        "agent" => {
            let agent = state
                .db
                .get_agent(&q.id)
                .await
                .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Agent not found"))?;
            if agent.account_id != auth.account_id {
                return Err(api_err(StatusCode::FORBIDDEN, "Access denied"));
            }
            agent.avatar_url
        }
        _ => return Err(api_err(StatusCode::BAD_REQUEST, "kind must be 'user' or 'agent'")),
    };

    let Some(url) = avatar_url.filter(|u| !u.trim().is_empty()) else {
        return Err(api_err(StatusCode::NOT_FOUND, "No avatar set"));
    };

    if let Some((content_type, bytes)) = decode_data_url(&url) {
        let mut resp = Response::new(Body::from(bytes));
        let ct = HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
        resp.headers_mut().insert(header::CONTENT_TYPE, ct);
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
        return Ok(resp);
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::FOUND;
        if let Ok(loc) = HeaderValue::from_str(&url) {
            resp.headers_mut().insert(header::LOCATION, loc);
        }
        return Ok(resp);
    }

    // Anything else (`file:`, relative, blob:, …) is refused: the sandbox could
    // never load it and it may point at local files.
    Err(api_err(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Avatar URL uses an unsupported scheme",
    ))
}

#[derive(Debug, Deserialize)]
pub struct StorageRequest {
    pub plugin_id: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
}

/// Resolve a plugin, its namespace and its grants in one place.
async fn storage_target(
    state: &AppState,
    auth: &AuthUser,
    plugin_id: &str,
) -> Result<(String, PluginPermissions), ApiError> {
    let server = load_owned_server(state, &auth.account_id, plugin_id).await?;
    Ok((
        storage_namespace(&auth.account_id, &server.id),
        server.get_permissions(),
    ))
}

/// `POST /api/host/storage/get`
pub async fn storage_get_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<StorageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = payload.key.unwrap_or_default();
    let (ns, perms) = storage_target(&state, &auth, &payload.plugin_id).await?;
    check_storage_access(&perms, "get", &key).map_err(|(c, m)| api_err(c, m))?;

    let value = state
        .db
        .get_extension_kv(&ns, &key)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "key": key, "value": value })))
}

/// `POST /api/host/storage/set`
pub async fn storage_set_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<StorageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = payload.key.unwrap_or_default();
    let value = payload.value.unwrap_or_default();
    let (ns, perms) = storage_target(&state, &auth, &payload.plugin_id).await?;
    check_storage_access(&perms, "set", &key).map_err(|(c, m)| api_err(c, m))?;

    state
        .db
        .set_extension_kv(&ns, &key, &value)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "key": key })))
}

/// `POST /api/host/storage/delete`
pub async fn storage_delete_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<StorageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = payload.key.unwrap_or_default();
    let (ns, perms) = storage_target(&state, &auth, &payload.plugin_id).await?;
    check_storage_access(&perms, "delete", &key).map_err(|(c, m)| api_err(c, m))?;

    let removed = state
        .db
        .delete_extension_kv(&ns, &key)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": removed, "key": key })))
}

/// `POST /api/host/storage/list`
pub async fn storage_list_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<StorageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (ns, perms) = storage_target(&state, &auth, &payload.plugin_id).await?;
    if !perms.allows(neuxdot_core::db::schema::CAP_STORAGE_KEYS) {
        return Err(api_err(
            StatusCode::FORBIDDEN,
            "capability 'storage.keys' is not granted",
        ));
    }

    let prefix = payload.prefix.unwrap_or_default();
    let items = state
        .db
        .list_extension_kv(&ns)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // A listed key must still sit inside the grant's allowed prefixes.
    let keys: Vec<String> = items
        .into_iter()
        .map(|kv| kv.key)
        .filter(|k| k.starts_with(&prefix))
        .filter(|k| perms.storage_allows_key(k))
        .collect();

    Ok(Json(json!({ "keys": keys })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use neuxdot_core::db::schema::{
        AgentPermissions, IdentityPermissions, StoragePermissions, UiPermissions,
    };

    fn perms() -> PluginPermissions {
        PluginPermissions {
            ui: UiPermissions { enabled: true, resources: vec!["ui://a".into()] },
            storage: StoragePermissions {
                get: true,
                set: true,
                delete: false,
                keys: Some(vec!["chat.".into()]),
            },
            agent: AgentPermissions { trigger: true, tools: false },
            identity: IdentityPermissions {
                agent_name: true,
                agent_profile_picture: false,
                user_name: true,
                user_profile_picture: false,
            },
            process: false,
            network: false,
        }
    }

    fn agent() -> Agent {
        Agent {
            id: "agent-1".into(),
            account_id: "acc".into(),
            name: "Research Assistant".into(),
            avatar_color: Some("#3b82f6".into()),
            avatar_shape: Some("circle".into()),
            avatar_url: Some("data:image/png;base64,AAAA".into()),
            strategy: "fallback".into(),
            system_prompt: None,
            is_active: true,
            allow_output_text: false,
            output_text_error_message: None,
            max_running_history_chars: 300_000,
            response_hooks_json: None,
            instruction_preset_id: None,
        }
    }

    #[test]
    fn ungranted_identity_fields_are_omitted_not_blanked() {
        let p = perms();
        let a = build_agent_identity(&agent(), &p).expect("agent block present");
        assert_eq!(a.name.as_deref(), Some("Research Assistant"));
        // Profile picture is NOT granted -> no avatar object at all.
        assert!(a.avatar.is_none());

        let user = build_user_identity(None, "ada", &p).expect("user block present");
        assert_eq!(user.name.as_deref(), Some("ada"));
        assert!(user.avatar.is_none());

        let serialised = serde_json::to_value(&user).unwrap();
        assert!(serialised.get("avatar").is_none(), "ungranted avatar must be omitted");
    }

    #[test]
    fn identity_block_and_keys_vanish_when_nothing_granted() {
        let mut p = perms();
        p.identity = IdentityPermissions::default();
        assert!(build_agent_identity(&agent(), &p).is_none());
        assert!(build_user_identity(None, "ada", &p).is_none());

        let boot = HostBridgeBootstrap {
            protocol_version: HOST_BRIDGE_PROTOCOL_VERSION.into(),
            app: AppDescriptor {
                plugin_id: "wasm:x".into(),
                plugin_name: "x".into(),
                resource_uri: "ui://a".into(),
                title: "x".into(),
            },
            agent: None,
            user: None,
            capabilities: p.capabilities(),
            permissions: p,
            context: BridgeContext { locale: None, theme: "light".into() },
        };
        let v = serde_json::to_value(&boot).unwrap();
        assert!(v.get("agent").is_none());
        assert!(v.get("user").is_none());
        // `storage.delete` is not granted in the fixture, and every identity
        // grant is off, so neither may appear in the advertised capabilities.
        assert_eq!(
            v["capabilities"],
            json!(["ui.context.refresh", "storage.get", "storage.set", "storage.keys", "agent.trigger"])
        );
        let caps = v["capabilities"].as_array().unwrap();
        assert!(caps.iter().all(|c| !c.as_str().unwrap().starts_with("identity.")));
    }

    #[test]
    fn storage_prefix_allow_list_is_enforced() {
        let p = perms();
        assert!(check_storage_access(&p, "get", "chat.draft").is_ok());
        assert!(check_storage_access(&p, "set", "chat.draft").is_ok());

        // Outside the allow-list -> 403.
        let err = check_storage_access(&p, "get", "secrets.token").unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        // Verb not granted -> 403.
        let err = check_storage_access(&p, "delete", "chat.draft").unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        // Empty key -> 400.
        let err = check_storage_access(&p, "get", "").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn storage_namespace_is_isolated_per_account_and_plugin() {
        let a = storage_namespace("acc1", "wasm:plugin");
        let b = storage_namespace("acc2", "wasm:plugin");
        let c = storage_namespace("acc1", "wasm:other");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, "host:acc1:plugin");
    }

    #[test]
    fn data_url_decoding() {
        // "hi" base64
        let (mime, bytes) = decode_data_url("data:text/plain;base64,aGk=").unwrap();
        assert_eq!(mime, "text/plain");
        assert_eq!(bytes, b"hi");

        let (mime, bytes) = decode_data_url("data:text/plain,hi").unwrap();
        assert_eq!(mime, "text/plain");
        assert_eq!(bytes, b"hi");

        assert!(decode_data_url("https://example.com/a.png").is_none());
        assert!(decode_data_url("file:///etc/passwd").is_none());
    }

}
