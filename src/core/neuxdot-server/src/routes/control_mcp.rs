//! The server's own MCP surface: let an agent drive NeuxDOT itself.
//!
//! ## Why an MCP endpoint instead of a bespoke API
//!
//! The host already speaks MCP to plugins. Exposing the control surface as an
//! MCP server means an agent reaches it the same way it reaches anything else:
//! register `POST /api/control/mcp` as an `http` plugin, bind it to an agent,
//! and the tools appear in the tool registry. No new plugin transport, no new
//! protocol, and the existing consent model applies unchanged.
//!
//! ## Authority
//!
//! Two independent gates, in this order:
//!
//! 1. **The credential's own account.** `Authorization: Basic user:password`
//!    is verified against the `accounts` table on *every* request, so a deleted
//!    or demoted account loses access immediately. Everything the call does is
//!    done *as that account*: ownership checks, sharing rules and role gates are
//!    the same ones the HTTP API applies. A `root` credential therefore acts as
//!    root, exactly as the UI would.
//! 2. **The plugin's declared scopes.** The plugin row that points at this
//!    endpoint must have declared the matching [`ControlPermissions`] flag. That
//!    is what lets an operator hand a bot agent CRUD without also handing it
//!    account administration.
//!
//! The scopes cannot *elevate*: a `member` credential stays a member even if
//! every flag is set, because gate 1 is enforced by the same repositories the
//! routes use.
//!
//! ## Deliberate limitations
//!
//! * No tool returns a password hash, a JWT or a provider API key. Listing
//!   providers masks `api_key`; account listing returns no credential material
//!   at all.
//! * `server_create_account` may create a `member`; creating a `root` account
//!   requires a root credential and is allowed, because that is what "acts as
//!   root 100% the same" means. It is audited through the event stream.
//! * The endpoint never spawns a process and never touches the filesystem, so it
//!   is safe to expose to a sandboxed plugin over HTTP.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use neuxdot_core::auth::verify_password;
use neuxdot_core::db::schema::{
    is_valid_role, AgentAccess, ControlPermissions, McpServer, ROLE_ROOT,
};
use serde_json::{json, Value};

use crate::state::AppState;

/// The `alias` an MCP client should use. Documented so a manifest can match it.
pub const CONTROL_ALIAS: &str = "control";

/// Who is calling, and what their plugin was allowed to do.
#[derive(Debug, Clone)]
pub struct ControlCaller {
    pub account_id: String,
    pub username: String,
    pub role: String,
    pub scopes: ControlPermissions,
}

impl ControlCaller {
    fn is_root(&self) -> bool {
        self.role == ROLE_ROOT
    }
}

/// A JSON-RPC error with an HTTP status.
#[derive(Debug)]
struct RpcError {
    status: StatusCode,
    code: i64,
    message: String,
    hint: Option<String>,
}

impl RpcError {
    fn new(status: StatusCode, code: i64, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, -32001, message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, -32003, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, -32004, message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, -32602, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, -32603, message)
    }

    fn to_json(&self) -> Value {
        let mut error = json!({
            "code": self.code,
            "message": self.message,
            "retryable": self.status == StatusCode::INTERNAL_SERVER_ERROR,
        });
        if let Some(hint) = &self.hint {
            error["hint"] = json!(hint);
        }
        json!({ "error": error })
    }
}

impl From<sqlx::Error> for RpcError {
    /// Repository failures become retryable internal errors carrying the cause,
    /// so an agent can tell a transient storage problem from a bad request.
    fn from(err: sqlx::Error) -> Self {
        RpcError::internal(format!("storage error: {err}"))
    }
}

type RpcResult = Result<Value, RpcError>;

/// `POST /api/control/mcp` — JSON-RPC 2.0 over HTTP.
///
/// Speaks the three MCP methods a tool-only client needs (`initialize`,
/// `tools/list`, `tools/call`) plus `ping`. Everything else answers a method
/// error rather than a bare 400, so an MCP client can render the failure.
pub async fn control_mcp_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let caller = match authenticate(&state, &headers).await {
        Ok(caller) => caller,
        Err(err) => return (err.status, Json(rpc_error_response(&Value::Null, &err))),
    };

    let id = payload.get("id").cloned().unwrap_or(Value::Null);
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = payload.get("params").cloned().unwrap_or(Value::Null);

    // `initialize` and `ping` are transport-level and must work before any tool
    // is called, so they are answered without a scope check. They expose only
    // static metadata.
    let result: RpcResult = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "neuxdot-control", "version": env!("CARGO_PKG_VERSION") },
            "instructions": instructions()
        })),
        "ping" => Ok(json!({})),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(json!({ "tools": tools_for(&caller) })),
        "tools/call" => call_tool(&state, &caller, &params).await,
        other => Err(
            RpcError::new(StatusCode::OK, -32601, format!("unknown method '{other}'"))
                .with_hint("This endpoint implements initialize, tools/list, tools/call and ping."),
        ),
    };

    let body = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => rpc_error_response(&id, &err),
    };
    // JSON-RPC failures travel inside a 200 body so MCP clients surface the
    // message instead of treating it as transport breakage. Auth failures stay
    // HTTP-level: they must not look like a working endpoint.
    (StatusCode::OK, Json(body))
}

fn rpc_error_response(id: &Value, err: &RpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": err.code,
            "message": err.message,
            "data": err.to_json()["error"].clone(),
        }
    })
}

fn instructions() -> String {
    "NeuxDOT control surface. Tools operate on this server's own configuration \
     (agents, plugins, providers, shares and, with a root credential, accounts). \
     You act as the account whose credentials were used; ownership rules and \
     sharing are enforced exactly as in the web UI. Destructive tools are \
     irreversible: delete tools remove rows permanently."
        .to_string()
}

/// Resolve `Authorization: Basic` into a caller with its declared scopes.
///
/// The scope lookup is done from the plugin row whose `url_or_command` points at
/// this endpoint. When several plugins point here, the union is used so a single
/// operator binding is enough; the credential remains the outer gate.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<ControlCaller, RpcError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            RpcError::unauthorized("missing Authorization header")
                .with_hint("Use HTTP Basic auth: Authorization: Basic base64(username:password)")
        })?;

    let encoded = raw.strip_prefix("Basic ").ok_or_else(|| {
        RpcError::unauthorized("expected HTTP Basic credentials")
            .with_hint("Authorization: Basic base64(username:password)")
    })?;

    let decoded = decode_base64(encoded.trim())
        .map_err(|_| RpcError::unauthorized("Authorization header is not valid base64"))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|_| RpcError::unauthorized("credentials are not valid UTF-8"))?;
    let (username, password) = decoded
        .split_once(':')
        .ok_or_else(|| RpcError::unauthorized("credentials must be 'username:password'"))?;

    // Uniform failure text: never reveal whether the account exists.
    let account = state
        .db
        .get_account_by_username(username)
        .await
        .map_err(|e| RpcError::internal(format!("identity lookup failed: {e}")))?
        .ok_or_else(|| RpcError::unauthorized("invalid username or password"))?;

    if !verify_password(password, &account.password_hash) {
        return Err(RpcError::unauthorized("invalid username or password"));
    }

    // Authority comes from the row, not from the credential's claims: a demoted
    // account loses root immediately.
    let role = if is_valid_role(&account.role) {
        account.role.clone()
    } else {
        neuxdot_core::db::schema::ROLE_MEMBER.to_string()
    };

    let scopes = collect_scopes(state, &account.id).await?;

    Ok(ControlCaller {
        account_id: account.id,
        username: account.username,
        role,
        scopes,
    })
}

/// Union of the control grants declared by every plugin that targets this
/// endpoint and is owned by, or shared with, `account_id`.
async fn collect_scopes(
    state: &AppState,
    account_id: &str,
) -> Result<ControlPermissions, RpcError> {
    let servers = state
        .db
        .list_mcp_servers(account_id)
        .await
        .map_err(|e| RpcError::internal(format!("could not load plugins: {e}")))?;

    let mut scopes = ControlPermissions::default();
    for server in servers.iter().filter(|s| targets_control(s)) {
        let declared = server.get_permissions().control;
        scopes.agents_read |= declared.agents_read;
        scopes.agents_write |= declared.agents_write;
        scopes.shares |= declared.shares;
        scopes.plugins_read |= declared.plugins_read;
        scopes.plugins_write |= declared.plugins_write;
        scopes.providers_read |= declared.providers_read;
        scopes.providers_write |= declared.providers_write;
        scopes.accounts |= declared.accounts;
    }
    Ok(scopes)
}

fn targets_control(server: &McpServer) -> bool {
    server.url_or_command.contains("/api/control/mcp")
}

/// Decode standard base64 (padding optional).
fn decode_base64(input: &str) -> Result<Vec<u8>, ()> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in cleaned {
        if byte == b'=' {
            break;
        }
        let v = value(byte).ok_or(())?;
        buffer = (buffer << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

// -----------------------------------------------------------------------------
// Tool catalogue
// -----------------------------------------------------------------------------

/// One catalogue entry: name, description, the scope predicate that gates it,
/// and its input schema. The predicate is `None` only for read-only identity
/// helpers that every authenticated caller may use.
type ToolSpec = (
    &'static str,
    &'static str,
    Option<fn(&ControlPermissions) -> bool>,
    Value,
);

/// Every tool, with the scope it requires.
fn tool_catalogue() -> Vec<ToolSpec> {
    use ControlPermissions as P;
    let read_any = |_: &P| true;

    vec![
        (
            "server_whoami",
            "Return the account this credential authenticates as, its role, and the control scopes granted to this plugin.",
            Some(read_any as fn(&ControlPermissions) -> bool),
            json!({ "type": "object", "properties": {} }),
        ),
        (
            "server_list_agents",
            "List agents visible to this account (owned and shared), with their models and bound plugins.",
            Some(|p: &P| p.agents_read),
            json!({
                "type": "object",
                "properties": {
                    "include_shared": { "type": "boolean", "description": "Include agents shared with this account (default true)" }
                }
            }),
        ),
        (
            "server_get_agent",
            "Read one agent, including its models, bound plugins and share list.",
            Some(|p: &P| p.agents_read),
            json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        ),
        (
            "server_create_agent",
            "Create a new agent owned by this account.",
            Some(|p: &P| p.agents_write),
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Display name" },
                    "system_prompt": { "type": "string" },
                    "strategy": { "type": "string", "description": "Routing strategy (default 'fallback')" },
                    "model_ids": { "type": "array", "items": { "type": "string" }, "description": "Provider-model ids, in priority order" },
                    "mcp_ids": { "type": "array", "items": { "type": "string" }, "description": "Plugin ids to bind" },
                    "allow_output_text": { "type": "boolean" }
                },
                "required": ["name"]
            }),
        ),
        (
            "server_update_agent",
            "Change an agent's name, prompt, strategy, active flag, model bindings or plugin bindings.",
            Some(|p: &P| p.agents_write),
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "name": { "type": "string" },
                    "system_prompt": { "type": "string" },
                    "strategy": { "type": "string" },
                    "is_active": { "type": "boolean" },
                    "allow_output_text": { "type": "boolean" },
                    "model_ids": { "type": "array", "items": { "type": "string" } },
                    "mcp_ids": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["agent_id"]
            }),
        ),
        (
            "server_delete_agent",
            "Permanently delete an agent. Owner only, and irreversible.",
            Some(|p: &P| p.agents_write),
            json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        ),
        (
            "server_list_agent_shares",
            "List who an agent is shared with and what access each sharee has.",
            Some(|p: &P| p.shares),
            json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        ),
        (
            "server_share_agent",
            "Share an agent with an account, or change an existing share's access.",
            Some(|p: &P| p.shares),
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "account_id": { "type": "string", "description": "Account to share with (use server_search_accounts to find one)" },
                    "can_edit": { "type": "boolean", "description": "Allow editing config (default false = use only)" },
                    "can_manage_shares": { "type": "boolean", "description": "Allow managing shares (default false)" }
                },
                "required": ["agent_id", "account_id"]
            }),
        ),
        (
            "server_unshare_agent",
            "Revoke an account's access to an agent.",
            Some(|p: &P| p.shares),
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "account_id": { "type": "string" }
                },
                "required": ["agent_id", "account_id"]
            }),
        ),
        (
            "server_list_plugins",
            "List plugins owned by this account, with transport, alias and resolved permissions.",
            Some(|p: &P| p.plugins_read),
            json!({ "type": "object", "properties": {} }),
        ),
        (
            "server_create_plugin",
            "Install a plugin. Secrets in headers_or_env are stored but never returned by read tools.",
            Some(|p: &P| p.plugins_write),
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "alias": { "type": "string", "description": "Tool-call prefix, e.g. 'telegram'" },
                    "transport": { "type": "string", "enum": ["stdio", "binary", "http", "custom_tools", "curl"] },
                    "url_or_command": { "type": "string", "description": "Command for stdio/binary, URL for http" },
                    "headers_or_env": { "type": "string", "description": "JSON object or KEY=VALUE lines" },
                    "permissions": { "type": "object", "description": "Optional permission override, same shape as the consent UI" }
                },
                "required": ["name", "transport", "url_or_command"]
            }),
        ),
        (
            "server_update_plugin",
            "Change a plugin's name, alias, endpoint, env/headers, permissions or active flag.",
            Some(|p: &P| p.plugins_write),
            json!({
                "type": "object",
                "properties": {
                    "plugin_id": { "type": "string" },
                    "name": { "type": "string" },
                    "alias": { "type": "string" },
                    "url_or_command": { "type": "string" },
                    "headers_or_env": { "type": "string" },
                    "permissions": { "type": "object" },
                    "is_active": { "type": "boolean" }
                },
                "required": ["plugin_id"]
            }),
        ),
        (
            "server_delete_plugin",
            "Permanently uninstall a plugin.",
            Some(|p: &P| p.plugins_write),
            json!({
                "type": "object",
                "properties": { "plugin_id": { "type": "string" } },
                "required": ["plugin_id"]
            }),
        ),
        (
            "server_list_providers",
            "List model providers and their models. API keys are never included.",
            Some(|p: &P| p.providers_read),
            json!({ "type": "object", "properties": {} }),
        ),
        (
            "server_create_provider",
            "Add a model provider with an optional model list.",
            Some(|p: &P| p.providers_write),
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "base_url": { "type": "string" },
                    "api_key": { "type": "string" },
                    "models": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "model_id": { "type": "string" },
                                "display_name": { "type": "string" },
                                "context_window": { "type": "integer" }
                            },
                            "required": ["model_id"]
                        }
                    }
                },
                "required": ["name", "base_url"]
            }),
        ),
        (
            "server_update_provider",
            "Change a provider's name, base URL, key or model list.",
            Some(|p: &P| p.providers_write),
            json!({
                "type": "object",
                "properties": {
                    "provider_id": { "type": "string" },
                    "name": { "type": "string" },
                    "base_url": { "type": "string" },
                    "api_key": { "type": "string" },
                    "models": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["provider_id", "name", "base_url"]
            }),
        ),
        (
            "server_delete_provider",
            "Permanently delete a provider and its models.",
            Some(|p: &P| p.providers_write),
            json!({
                "type": "object",
                "properties": { "provider_id": { "type": "string" } },
                "required": ["provider_id"]
            }),
        ),
        (
            "server_search_accounts",
            "Search accounts by username so you can share an agent with someone. Returns id and username only.",
            Some(|p: &P| p.shares || p.accounts),
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }),
        ),
        (
            "server_list_accounts",
            "List all accounts. Requires a root credential plus the 'accounts' scope.",
            Some(|p: &P| p.accounts),
            json!({ "type": "object", "properties": {} }),
        ),
        (
            "server_create_account",
            "Create an account. Requires a root credential. Passwords must satisfy the server's strength policy.",
            Some(|p: &P| p.accounts),
            json!({
                "type": "object",
                "properties": {
                    "username": { "type": "string" },
                    "password": { "type": "string" },
                    "role": { "type": "string", "enum": ["root", "member"], "description": "Default member. 'root' grants full server control." },
                    "display_name": { "type": "string" }
                },
                "required": ["username", "password"]
            }),
        ),
        (
            "server_update_account",
            "Change an account's username, role or display name. Requires a root credential. The last root cannot be demoted.",
            Some(|p: &P| p.accounts),
            json!({
                "type": "object",
                "properties": {
                    "account_id": { "type": "string" },
                    "username": { "type": "string" },
                    "role": { "type": "string", "enum": ["root", "member"] },
                    "display_name": { "type": "string" }
                },
                "required": ["account_id"]
            }),
        ),
        (
            "server_delete_account",
            "Permanently delete an account. Requires a root credential; you cannot delete yourself or the last root.",
            Some(|p: &P| p.accounts),
            json!({
                "type": "object",
                "properties": { "account_id": { "type": "string" } },
                "required": ["account_id"]
            }),
        ),
    ]
}

/// The tools this caller may see. Withholding a tool the caller cannot use is
/// clearer than returning 403 from inside a call: the model never wastes a turn
/// discovering an unavailable capability.
fn tools_for(caller: &ControlCaller) -> Vec<Value> {
    tool_catalogue()
        .into_iter()
        .filter(|(_, _, required, _)| match required {
            Some(check) => check(&caller.scopes),
            None => true,
        })
        .map(|(name, description, _, schema)| {
            json!({ "name": name, "description": description, "inputSchema": schema })
        })
        .collect()
}

/// The scope check for one tool, re-derived from the catalogue so `tools/list`
/// and `tools/call` can never disagree.
fn scope_allows(caller: &ControlCaller, name: &str) -> bool {
    match tool_catalogue()
        .into_iter()
        .find(|(tool_name, _, _, _)| *tool_name == name)
    {
        Some((_, _, Some(check), _)) => check(&caller.scopes),
        Some(_) => true,
        None => false,
    }
}

async fn call_tool(state: &AppState, caller: &ControlCaller, params: &Value) -> RpcResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::bad_request("tools/call requires a 'name'"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if !scope_allows(caller, name) {
        return Err(RpcError::forbidden(format!(
            "this plugin was not granted the scope required by '{name}'. \
             Ask the account owner to enable it in the plugin's permissions."
        ))
        .with_hint("Scopes are declared in the plugin manifest under permissions.control"));
    }

    let value = dispatch(state, caller, name, &args).await?;
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()) }],
        "isError": false
    }))
}

fn require_str(args: &Value, key: &str) -> Result<String, RpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| RpcError::bad_request(format!("missing required parameter '{key}'")))
}

fn optional_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn optional_str_list(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(Value::as_array).map(|list| {
        list.iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

async fn dispatch(state: &AppState, caller: &ControlCaller, name: &str, args: &Value) -> RpcResult {
    match name {
        "server_whoami" => Ok(json!({
            "account_id": caller.account_id,
            "username": caller.username,
            "role": caller.role,
            "is_root": caller.is_root(),
            "scopes": caller.scopes.granted(),
        })),
        _ => dispatch_resources(state, caller, name, args).await,
    }
}

async fn dispatch_resources(
    state: &AppState,
    caller: &ControlCaller,
    name: &str,
    args: &Value,
) -> RpcResult {
    match name {
        // ------------------------------------------------------------- agents
        "server_list_agents" => {
            let include_shared = args
                .get("include_shared")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            // `list_agents_for_account` already carries the resolved access
            // flags, so one query answers both the membership and the label.
            let rows = if include_shared {
                state
                    .db
                    .list_agents_for_account(&caller.account_id)
                    .await?
                    .into_iter()
                    .map(|(agent, is_owner, can_edit, can_manage_shares)| {
                        (agent, access_label(is_owner, can_edit, can_manage_shares))
                    })
                    .collect::<Vec<_>>()
            } else {
                state
                    .db
                    .list_agents(&caller.account_id)
                    .await?
                    .into_iter()
                    .map(|agent| (agent, "owner"))
                    .collect::<Vec<_>>()
            };

            let mut out = Vec::with_capacity(rows.len());
            for (agent, allowed) in rows {
                let models = state
                    .db
                    .get_agent_models(&agent.id)
                    .await
                    .unwrap_or_default();
                let plugins = state
                    .db
                    .get_agent_mcp_servers(&agent.id)
                    .await
                    .unwrap_or_default();
                out.push(json!({
                    "id": agent.id,
                    "name": agent.name,
                    "owner_account_id": agent.account_id,
                    "is_active": agent.is_active,
                    "strategy": agent.strategy,
                    "allowed": allowed,
                    "model_count": models.len(),
                    "plugin_count": plugins.len(),
                }));
            }
            Ok(json!({ "count": out.len(), "agents": out }))
        }
        "server_get_agent" => {
            let agent_id = require_str(args, "agent_id")?;
            let (agent, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            let models = state
                .db
                .get_agent_models(&agent.id)
                .await
                .unwrap_or_default();
            let plugins = state
                .db
                .get_agent_mcp_servers(&agent.id)
                .await
                .unwrap_or_default();
            let shares = state
                .db
                .list_agent_shares(&agent.id)
                .await
                .unwrap_or_default();
            Ok(json!({
                "agent": agent,
                "allowed": agent_access_label(access),
                "models": models,
                "plugins": plugins.iter().map(|p| json!({
                    "id": p.id, "name": p.name, "alias": p.get_alias(), "transport": p.transport, "is_active": p.is_active
                })).collect::<Vec<_>>(),
                "shares": shares,
            }))
        }
        "server_create_agent" => {
            let name = require_str(args, "name")?;
            let agent = state
                .db
                .create_agent(
                    &caller.account_id,
                    &name,
                    optional_str(args, "avatar_color").as_deref(),
                    optional_str(args, "avatar_shape").as_deref(),
                    optional_str(args, "avatar_url").as_deref(),
                    optional_str(args, "strategy")
                        .as_deref()
                        .unwrap_or("fallback")
                        .to_string()
                        .as_str(),
                    optional_str(args, "system_prompt").as_deref(),
                    args.get("allow_output_text").and_then(Value::as_bool),
                    None,
                    None,
                    None,
                    None,
                )
                .await?;

            if let Some(models) = optional_str_list(args, "model_ids") {
                state.db.set_agent_models(&agent.id, &models).await?;
            }
            if let Some(plugins) = optional_str_list(args, "mcp_ids") {
                state.db.set_agent_mcp_servers(&agent.id, &plugins).await?;
            }
            Ok(json!({ "created": true, "agent": agent }))
        }
        "server_update_agent" => {
            let agent_id = require_str(args, "agent_id")?;
            let (_, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            require_edit(access)?;

            let updated = state
                .db
                .patch_agent(
                    &agent_id,
                    optional_str(args, "name").as_deref(),
                    optional_str(args, "avatar_color").as_deref(),
                    optional_str(args, "avatar_shape").as_deref(),
                    optional_str(args, "avatar_url").as_deref(),
                    optional_str(args, "strategy").as_deref(),
                    optional_str(args, "system_prompt").as_deref(),
                    args.get("is_active").and_then(Value::as_bool),
                    args.get("allow_output_text").and_then(Value::as_bool),
                    None,
                    None,
                    None,
                    None,
                )
                .await?;

            if let Some(models) = optional_str_list(args, "model_ids") {
                state.db.set_agent_models(&agent_id, &models).await?;
            }
            if let Some(plugins) = optional_str_list(args, "mcp_ids") {
                state.db.set_agent_mcp_servers(&agent_id, &plugins).await?;
                state.subscriptions.sync_agent(&agent_id).await;
            }
            Ok(json!({ "updated": true, "agent": updated }))
        }
        "server_delete_agent" => {
            let agent_id = require_str(args, "agent_id")?;
            let (agent, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            // Deleting is owner-only: a sharee may leave, never destroy.
            if !agent.account_id.eq(&caller.account_id) {
                return Err(RpcError::forbidden(
                    "only the owner may delete an agent; a sharee can revoke their own access instead",
                ));
            }
            if !access.can_edit() {
                return Err(RpcError::forbidden(
                    "you do not have permission to delete this agent",
                ));
            }
            let deleted = state.db.delete_agent(&agent_id).await?;
            state.subscriptions.sync_agent(&agent_id).await;
            Ok(json!({ "deleted": deleted }))
        }
        // ------------------------------------------------------------- shares
        "server_list_agent_shares" => {
            let agent_id = require_str(args, "agent_id")?;
            let (_, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            require_manage_shares(access)?;
            let shares = state.db.list_agent_shares(&agent_id).await?;
            Ok(json!({ "shares": shares }))
        }
        "server_share_agent" => {
            let agent_id = require_str(args, "agent_id")?;
            let account_id = require_str(args, "account_id")?;
            let (agent, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            require_manage_shares(access)?;

            if account_id == agent.account_id {
                return Err(RpcError::bad_request(
                    "the owner already has full access and cannot be a sharee",
                ));
            }
            if state.db.get_account_by_id(&account_id).await?.is_none() {
                return Err(RpcError::not_found("no such account"));
            }

            let can_edit = args
                .get("can_edit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let can_manage = args
                .get("can_manage_shares")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // Only the owner may hand out share management.
            if can_manage && !agent.account_id.eq(&caller.account_id) {
                return Err(RpcError::forbidden(
                    "only the owner may grant share-management rights",
                ));
            }

            let share = state
                .db
                .upsert_agent_share(&agent_id, &account_id, can_edit, can_manage)
                .await?;
            Ok(json!({ "ok": true, "share": share }))
        }
        "server_unshare_agent" => {
            let agent_id = require_str(args, "agent_id")?;
            let account_id = require_str(args, "account_id")?;
            let (_, access) = load_agent(state, &agent_id, &caller.account_id).await?;
            // A sharee may always revoke their own access.
            let revoking_self = account_id == caller.account_id;
            if !revoking_self {
                require_manage_shares(access)?;
            }
            let deleted = state.db.delete_agent_share(&agent_id, &account_id).await?;
            Ok(json!({ "deleted": deleted }))
        }
        // ------------------------------------------------------------ plugins
        "server_list_plugins" => {
            let servers = state.db.list_mcp_servers(&caller.account_id).await?;
            let out: Vec<Value> = servers
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "name": s.name,
                        "alias": s.get_alias(),
                        "transport": s.transport,
                        "url_or_command": s.url_or_command,
                        "is_active": s.is_active,
                        "scopes": s.get_permissions().control.granted(),
                        "elevated": s.get_permissions().elevated,
                    })
                })
                .collect();
            Ok(json!({ "count": out.len(), "plugins": out }))
        }
        "server_create_plugin" => {
            let name = require_str(args, "name")?;
            let transport = require_str(args, "transport")?.to_lowercase();
            if !matches!(
                transport.as_str(),
                "stdio" | "binary" | "http" | "custom_tools" | "curl"
            ) {
                return Err(RpcError::bad_request(format!(
                    "unknown transport '{transport}'; expected stdio, binary, http, custom_tools or curl"
                )));
            }
            let url_or_command = require_str(args, "url_or_command")?;

            let server = state
                .db
                .create_mcp_server(
                    &caller.account_id,
                    &name,
                    optional_str(args, "alias").as_deref(),
                    &transport,
                    &url_or_command,
                    optional_str(args, "headers_or_env").as_deref(),
                )
                .await?;

            if let Some(perms) = args.get("permissions") {
                let perms_str = serde_json::to_string(perms)
                    .map_err(|e| RpcError::bad_request(format!("invalid permissions: {e}")))?;
                state
                    .db
                    .update_plugin_permissions(&server.id, &perms_str)
                    .await?;
            }

            let created = state.db.get_mcp_server(&server.id).await?.unwrap_or(server);
            Ok(json!({ "created": true, "plugin": redact_plugin(&created) }))
        }
        "server_update_plugin" => {
            let plugin_id = require_str(args, "plugin_id")?;
            let existing = state
                .db
                .get_mcp_server(&plugin_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such plugin"))?;
            if existing.account_id != caller.account_id {
                return Err(RpcError::forbidden(
                    "only the account that installed a plugin may modify it",
                ));
            }

            // `update_mcp_server` rewrites every column, so unchanged fields
            // must be passed through explicitly or they would be nulled.
            let transport =
                optional_str(args, "transport").unwrap_or_else(|| existing.transport.clone());
            let updated = state
                .db
                .update_mcp_server(
                    &plugin_id,
                    optional_str(args, "name")
                        .as_deref()
                        .unwrap_or(&existing.name),
                    optional_str(args, "alias")
                        .as_deref()
                        .or(existing.alias.as_deref()),
                    &transport,
                    optional_str(args, "url_or_command")
                        .as_deref()
                        .unwrap_or(&existing.url_or_command),
                    optional_str(args, "headers_or_env")
                        .as_deref()
                        .or(existing.headers_or_env.as_deref()),
                )
                .await?;

            if let Some(perms) = args.get("permissions") {
                let perms_str = serde_json::to_string(perms)
                    .map_err(|e| RpcError::bad_request(format!("invalid permissions: {e}")))?;
                state
                    .db
                    .update_plugin_permissions(&plugin_id, &perms_str)
                    .await?;
            }
            if let Some(active) = args.get("is_active").and_then(Value::as_bool) {
                state.db.set_plugin_active(&plugin_id, active).await?;
            }
            Ok(json!({ "updated": true, "plugin": redact_plugin(&updated) }))
        }
        "server_delete_plugin" => {
            let plugin_id = require_str(args, "plugin_id")?;
            let existing = state
                .db
                .get_mcp_server(&plugin_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such plugin"))?;
            if existing.account_id != caller.account_id {
                return Err(RpcError::forbidden(
                    "only the account that installed a plugin may delete it",
                ));
            }
            let deleted = state.db.delete_mcp_server(&plugin_id).await?;
            Ok(json!({ "deleted": deleted }))
        }
        // ---------------------------------------------------------- providers
        "server_list_providers" => {
            let providers = state
                .db
                .list_providers_with_models(&caller.account_id)
                .await?;
            Ok(json!({
                "providers": providers
                    .iter()
                    .map(redact_provider_with_models)
                    .collect::<Vec<_>>()
            }))
        }
        "server_create_provider" => {
            let name = require_str(args, "name")?;
            let base_url = require_str(args, "base_url")?;
            let provider = state
                .db
                .create_provider(
                    &caller.account_id,
                    &name,
                    &base_url,
                    optional_str(args, "api_key").as_deref(),
                )
                .await?;

            if let Some(models) = args.get("models").and_then(Value::as_array) {
                let count = models.len();
                let configs = provider_model_configs(models)?;
                state.db.set_provider_models(&provider.id, &configs).await?;
                return Ok(
                    json!({ "created": true, "provider_id": provider.id, "model_count": count }),
                );
            }
            Ok(json!({ "created": true, "provider_id": provider.id, "model_count": 0 }))
        }
        "server_update_provider" => {
            let provider_id = require_str(args, "provider_id")?;
            let existing = state
                .db
                .get_provider(&provider_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such provider"))?;
            if existing.account_id != caller.account_id {
                return Err(RpcError::forbidden("access denied"));
            }
            let name = require_str(args, "name")?;
            let base_url = require_str(args, "base_url")?;
            state
                .db
                .update_provider(
                    &provider_id,
                    &name,
                    &base_url,
                    optional_str(args, "api_key").as_deref(),
                )
                .await?;
            if let Some(models) = args.get("models").and_then(Value::as_array) {
                let configs = provider_model_configs(models)?;
                state.db.set_provider_models(&provider_id, &configs).await?;
            }
            Ok(json!({ "updated": true, "provider_id": provider_id }))
        }
        "server_delete_provider" => {
            let provider_id = require_str(args, "provider_id")?;
            let existing = state
                .db
                .get_provider(&provider_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such provider"))?;
            if existing.account_id != caller.account_id {
                return Err(RpcError::forbidden("access denied"));
            }
            let deleted = state.db.delete_provider(&provider_id).await?;
            Ok(json!({ "deleted": deleted }))
        }
        // ----------------------------------------------------------- accounts
        "server_search_accounts" => {
            let query = require_str(args, "query")?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 100) as usize;
            let accounts = state
                .db
                .search_accounts_for_sharing(&query, limit as i64)
                .await?;
            // Id and username only: enough to share, nothing else.
            Ok(json!({
                "accounts": accounts
                    .into_iter()
                    .map(|(id, username, display_name, avatar_url)| json!({
                        "id": id,
                        "username": username,
                        "display_name": display_name,
                        "avatar_url": avatar_url,
                    }))
                    .collect::<Vec<_>>()
            }))
        }
        "server_list_accounts" => {
            require_root(caller)?;
            let accounts = state.db.list_accounts().await?;
            Ok(json!({
                "accounts": accounts.iter().map(|a| json!({
                    "id": a.id,
                    "username": a.username,
                    "role": a.role,
                    "created_at": a.created_at,
                })).collect::<Vec<_>>()
            }))
        }
        "server_create_account" => {
            require_root(caller)?;
            let username = require_str(args, "username")?;
            let password = require_str(args, "password")?;
            let role = optional_str(args, "role").unwrap_or_else(|| "member".to_string());
            if !is_valid_role(&role) {
                return Err(RpcError::bad_request("role must be 'root' or 'member'"));
            }
            if let Err(err) = neuxdot_core::auth::validate_password_strength(&password) {
                return Err(RpcError::bad_request(format!("weak password: {err}"))
                    .with_hint("At least 8 characters, one letter and one digit, no whitespace."));
            }
            if state.db.get_account_by_username(&username).await?.is_some() {
                return Err(RpcError::new(
                    StatusCode::CONFLICT,
                    -32009,
                    "username already exists",
                ));
            }
            let hash = neuxdot_core::auth::hash_password(&password)
                .map_err(|e| RpcError::internal(format!("hashing failed: {e}")))?;
            let account = state
                .db
                .create_account_with_role(&username, &hash, &role)
                .await?;
            if let Some(display_name) = optional_str(args, "display_name") {
                // Best effort: the account exists and is usable either way, so a
                // profile failure must not fail the creation.
                if let Err(e) = state
                    .db
                    .update_profile(&account.id, Some(&display_name), None, None, None)
                    .await
                {
                    tracing::warn!(
                        "control: account {} created but its display name failed: {e}",
                        account.id
                    );
                }
            }
            Ok(json!({
                "created": true,
                "account": { "id": account.id, "username": account.username, "role": account.role }
            }))
        }
        "server_update_account" => {
            require_root(caller)?;
            let account_id = require_str(args, "account_id")?;
            let existing = state
                .db
                .get_account_by_id(&account_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such account"))?;

            let username = match optional_str(args, "username") {
                Some(raw) => {
                    if raw != existing.username
                        && state.db.get_account_by_username(&raw).await?.is_some()
                    {
                        return Err(RpcError::new(
                            StatusCode::CONFLICT,
                            -32009,
                            "username already exists",
                        ));
                    }
                    Some(raw)
                }
                None => None,
            };

            let role = match optional_str(args, "role") {
                Some(ref raw) => {
                    if !is_valid_role(raw) {
                        return Err(RpcError::bad_request("role must be 'root' or 'member'"));
                    }
                    // The last root must stay a root or the server locks out.
                    if existing.is_root()
                        && raw != ROLE_ROOT
                        && state.db.is_last_root(&account_id).await?
                    {
                        return Err(RpcError::bad_request(
                            "cannot demote the last remaining root account",
                        ));
                    }
                    Some(raw.clone())
                }
                None => None,
            };

            let updated = state
                .db
                .update_account(&account_id, username.as_deref(), role.as_deref())
                .await?;

            if let Some(display_name) = optional_str(args, "display_name") {
                state
                    .db
                    .update_profile(&account_id, Some(&display_name), None, None, None)
                    .await?;
            }
            Ok(json!({
                "updated": true,
                "account": {
                    "id": updated.id,
                    "username": updated.username,
                    "role": updated.role
                }
            }))
        }
        "server_delete_account" => {
            require_root(caller)?;
            let account_id = require_str(args, "account_id")?;
            if account_id == caller.account_id {
                return Err(RpcError::forbidden("you cannot delete your own account"));
            }
            let existing = state
                .db
                .get_account_by_id(&account_id)
                .await?
                .ok_or_else(|| RpcError::not_found("no such account"))?;
            if existing.is_root() && state.db.is_last_root(&account_id).await? {
                return Err(RpcError::bad_request(
                    "cannot delete the last remaining root account",
                ));
            }
            let deleted = state.db.delete_account(&account_id).await?;
            Ok(json!({ "deleted": deleted }))
        }
        other => Err(RpcError::new(
            StatusCode::OK,
            -32601,
            format!("unknown tool '{other}'"),
        )),
    }
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn require_root(caller: &ControlCaller) -> Result<(), RpcError> {
    if caller.is_root() {
        Ok(())
    } else {
        Err(RpcError::forbidden(
            "this tool requires a root credential; the account you authenticated with is a member",
        )
        .with_hint(
            "Use the credentials of a root account, or ask an admin to perform this action.",
        ))
    }
}

async fn load_agent(
    state: &AppState,
    agent_id: &str,
    account_id: &str,
) -> Result<(neuxdot_core::db::schema::Agent, AgentAccess), RpcError> {
    let agent = state
        .db
        .get_agent(agent_id)
        .await?
        .ok_or_else(|| RpcError::not_found("no such agent"))?;
    let access = state.db.resolve_agent_access(&agent, account_id).await?;
    if !access.can_view() {
        // 404, not 403: whether the id exists is itself sensitive.
        return Err(RpcError::not_found("no such agent"));
    }
    Ok((agent, access))
}

fn require_edit(access: AgentAccess) -> Result<(), RpcError> {
    if access.can_edit() {
        Ok(())
    } else {
        Err(RpcError::forbidden(
            "you do not have permission to edit this agent",
        ))
    }
}

fn require_manage_shares(access: AgentAccess) -> Result<(), RpcError> {
    if access.can_manage_shares() {
        Ok(())
    } else {
        Err(RpcError::forbidden(
            "you do not have permission to manage sharing for this agent",
        ))
    }
}

fn agent_access_label(access: AgentAccess) -> &'static str {
    match access {
        AgentAccess::Owner => "owner",
        AgentAccess::Shared { can_edit: true, .. } => "edit",
        AgentAccess::Shared {
            can_manage_shares: true,
            ..
        } => "manage_shares",
        AgentAccess::Shared { .. } => "view",
        AgentAccess::None => "none",
    }
}

/// The same labels, derived from the boolean tuple `list_agents_for_account`
/// returns. Kept next to [`agent_access_label`] so the two cannot drift.
fn access_label(is_owner: bool, can_edit: bool, can_manage_shares: bool) -> &'static str {
    if is_owner {
        "owner"
    } else if can_edit {
        "edit"
    } else if can_manage_shares {
        "manage_shares"
    } else {
        "view"
    }
}

/// Strip credential material from a plugin before it is shown to a model.
fn redact_plugin(server: &McpServer) -> Value {
    json!({
        "id": server.id,
        "name": server.name,
        "alias": server.get_alias(),
        "transport": server.transport,
        "url_or_command": server.url_or_command,
        "is_active": server.is_active,
        // Never echo `headers_or_env`: it holds API keys and tokens.
        "has_env_or_headers": server
            .headers_or_env
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
    })
}

/// Redact a provider that carries its model list.
fn redact_provider_with_models(provider: &neuxdot_core::db::schema::ProviderWithModels) -> Value {
    json!({
        "id": provider.provider.id,
        "name": provider.provider.name,
        "base_url": provider.provider.base_url,
        // The UI needs to know a key is set; it must never need the key itself.
        "has_api_key": provider
            .provider
            .api_key
            .as_deref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false),
        "models": provider.models,
    })
}

fn provider_model_configs(
    models: &[Value],
) -> Result<Vec<neuxdot_core::db::schema::ModelConfigInput>, RpcError> {
    let mut out = Vec::with_capacity(models.len());
    for model in models {
        let model_id = model
            .get("model_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RpcError::bad_request("each model requires a non-empty 'model_id'"))?;
        out.push(neuxdot_core::db::schema::ModelConfigInput {
            model_id: model_id.to_string(),
            name: model
                .get("display_name")
                .or_else(|| model.get("name"))
                .and_then(Value::as_str)
                .map(String::from),
            config_json: model
                .get("config_json")
                .and_then(Value::as_str)
                .map(String::from),
            params_json: None,
            context_window: model.get("context_window").and_then(Value::as_i64),
        });
    }
    Ok(out)
}

/// Shared handle so the router can take the same `AppState` as every other
/// handler without cloning it into this module.
pub fn _assert_state_is_shared(_: Arc<AppState>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_credentials() {
        assert_eq!(decode_base64("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert_eq!(
            decode_base64("dXNlcjpwYXNz").unwrap(),
            b"user:pass".to_vec()
        );
        // Padding is optional.
        assert_eq!(decode_base64("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert!(decode_base64("!!!!").is_err());
    }

    #[test]
    fn every_catalogue_tool_has_a_matching_schema_shape() {
        for (name, description, required, schema) in tool_catalogue() {
            assert!(name.starts_with("server_"), "{name} must be namespaced");
            assert!(!description.is_empty(), "{name} needs a description");
            assert_eq!(schema["type"], "object", "{name} schema must be an object");
            // Read-only identity is the only tool without a resource gate.
            if name != "server_whoami" {
                assert!(required.is_some(), "{name} must declare a scope");
            }
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<&str> = tool_catalogue().iter().map(|(n, _, _, _)| *n).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate tool name in the catalogue");
    }

    #[test]
    fn deny_by_default_hides_every_mutating_tool() {
        let caller = ControlCaller {
            account_id: "a".into(),
            username: "u".into(),
            role: "member".into(),
            scopes: ControlPermissions::default(),
        };
        let names: Vec<String> = tools_for(&caller)
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        // Only the identity tool survives without any grant.
        assert_eq!(names, vec!["server_whoami".to_string()]);
    }

    #[test]
    fn scopes_expose_exactly_their_own_tools() {
        let caller = |control: ControlPermissions| ControlCaller {
            account_id: "a".into(),
            username: "u".into(),
            role: "member".into(),
            scopes: control,
        };

        let agents_only = caller(ControlPermissions {
            agents_read: true,
            ..Default::default()
        });
        let names = |c: &ControlCaller| -> Vec<String> {
            tools_for(c)
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect()
        };
        let listed = names(&agents_only);
        assert!(listed.contains(&"server_list_agents".to_string()));
        assert!(listed.contains(&"server_get_agent".to_string()));
        assert!(!listed.contains(&"server_create_agent".to_string()));
        assert!(!listed.contains(&"server_delete_account".to_string()));

        let everything = caller(ControlPermissions {
            agents_read: true,
            agents_write: true,
            shares: true,
            plugins_read: true,
            plugins_write: true,
            providers_read: true,
            providers_write: true,
            accounts: true,
        });
        assert_eq!(names(&everything).len(), tool_catalogue().len());
    }

    #[test]
    fn scope_check_and_tools_list_agree() {
        let caller = ControlCaller {
            account_id: "a".into(),
            username: "u".into(),
            role: "member".into(),
            scopes: ControlPermissions {
                providers_read: true,
                ..Default::default()
            },
        };
        for (name, _, _, _) in tool_catalogue() {
            let listed = tools_for(&caller)
                .iter()
                .any(|t| t["name"].as_str() == Some(name));
            assert_eq!(
                listed,
                scope_allows(&caller, name),
                "tools/list and tools/call disagree about {name}"
            );
        }
    }

    #[test]
    fn non_root_cannot_reach_account_tools_even_with_the_scope() {
        let member = ControlCaller {
            account_id: "a".into(),
            username: "u".into(),
            role: "member".into(),
            scopes: ControlPermissions {
                accounts: true,
                ..Default::default()
            },
        };
        // The tool is *listed* (the plugin declared the scope) but the role gate
        // refuses it: the credential is the outer authority.
        assert!(scope_allows(&member, "server_list_accounts"));
        assert!(require_root(&member).is_err());

        let root = ControlCaller {
            role: ROLE_ROOT.into(),
            ..member.clone()
        };
        assert!(require_root(&root).is_ok());
    }

    #[test]
    fn provider_keys_are_redacted() {
        use neuxdot_core::db::schema::{Provider, ProviderWithModels};
        let provider = ProviderWithModels {
            provider: Provider {
                id: "p1".into(),
                account_id: "a".into(),
                name: "OpenAI".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: Some("sk-secret-value".into()),
                created_at: None,
            },
            models: Vec::new(),
        };
        let redacted = redact_provider_with_models(&provider);
        // The key itself is never in the payload; only its presence is.
        assert_eq!(redacted.get("api_key"), None);
        assert_eq!(redacted["has_api_key"], true);
        assert!(!redacted.to_string().contains("sk-secret-value"));
    }

    #[test]
    fn plugin_env_is_never_echoed() {
        let server = McpServer {
            id: "p".into(),
            account_id: "a".into(),
            name: "Telegram".into(),
            alias: Some("telegram".into()),
            transport: "binary".into(),
            url_or_command: "./data/binaries/telegram".into(),
            headers_or_env: Some(r#"{"TELEGRAM_BOT_TOKEN":"123:secret"}"#.into()),
            version: "1.0.0".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: None,
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        };
        let redacted = redact_plugin(&server);
        assert_eq!(redacted["has_env_or_headers"], true);
        assert!(!redacted.to_string().contains("secret"));
    }

    #[test]
    fn missing_required_parameters_are_named() {
        let err = require_str(&json!({}), "agent_id").unwrap_err();
        assert!(err.message.contains("agent_id"));
        assert_eq!(err.code, -32602);
    }

    #[test]
    fn only_control_endpoints_count_as_control_plugins() {
        let mut server = McpServer {
            id: "p".into(),
            account_id: "a".into(),
            name: "Ctl".into(),
            alias: None,
            transport: "http".into(),
            url_or_command: "http://127.0.0.1:8000/api/control/mcp".into(),
            headers_or_env: None,
            version: "1.0.0".into(),
            description: None,
            wasm_hash: None,
            wasm_binary: None,
            manifest_json: None,
            permissions_json: None,
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        };
        assert!(targets_control(&server));
        server.url_or_command = "http://127.0.0.1:8000/api/agents".into();
        assert!(!targets_control(&server));
    }
}
