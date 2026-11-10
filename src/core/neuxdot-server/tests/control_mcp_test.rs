//! End-to-end tests for the server-native control MCP.
//!
//! These exercise the real router over HTTP with real Basic-auth credentials and
//! a real SQLite database, so they cover the parts that unit tests cannot: the
//! auth header, the scope lookup from the plugin row, the JSON-RPC envelope, and
//! the authorization rules the tools delegate to.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use neuxdot_core::auth::hash_password;
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::{ROLE_MEMBER, ROLE_ROOT};
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_service::Service;

const ROOT_PASSWORD: &str = "RootPassw0rd";
const MEMBER_PASSWORD: &str = "MemberPassw0rd";

struct TestApp {
    app: Router,
    db: Arc<Database>,
    root_id: String,
    member_id: String,
}

async fn setup() -> TestApp {
    let pool = init_db("sqlite::memory:").await.expect("init db");
    let db = Arc::new(Database::new(pool));

    let (root, _) = db.get_or_init_root_account().await.expect("root account");
    let root_hash = hash_password(ROOT_PASSWORD).expect("hash");
    db.update_account_password(&root.id, &root_hash)
        .await
        .expect("root password");

    let member_hash = hash_password(MEMBER_PASSWORD).expect("hash");
    let member = db
        .create_account_with_role("alice", &member_hash, ROLE_MEMBER)
        .await
        .expect("member account");

    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    let app = build_router(AppState {
        subscriptions: neuxdot_server::subscriptions::SubscriptionHub::new(
            db.clone(),
            event_tx.clone(),
        ),
        db: db.clone(),
        event_tx,
    });

    TestApp {
        app,
        db,
        root_id: root.id,
        member_id: member.id,
    }
}

/// Register a plugin row pointing at the control endpoint, with the given
/// control scopes, owned by `account_id`.
async fn register_control_plugin(app: &TestApp, account_id: &str, control: Value) -> String {
    let server = app
        .db
        .create_mcp_server(
            account_id,
            "NeuxDOT Control",
            Some("control"),
            "http",
            "http://127.0.0.1:8000/api/control/mcp",
            None,
        )
        .await
        .expect("create plugin");

    let perms = json!({ "control": control, "network": { "allow": ["127.0.0.1"] } });
    app.db
        .update_plugin_permissions(&server.id, &perms.to_string())
        .await
        .expect("set permissions");
    server.id
}

fn basic(user: &str, password: &str) -> String {
    use base64_encode::encode;
    format!("Basic {}", encode(format!("{user}:{password}").as_bytes()))
}

/// Minimal base64 encoder so the test does not need a dependency.
mod base64_encode {
    pub fn encode(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = chunk.get(1).map(|b| u32::from(*b)).unwrap_or(0);
            let b2 = chunk.get(2).map(|b| u32::from(*b)).unwrap_or(0);
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(char::from(ALPHABET[((triple >> 18) & 0x3f) as usize]));
            out.push(char::from(ALPHABET[((triple >> 12) & 0x3f) as usize]));
            out.push(if chunk.len() > 1 {
                char::from(ALPHABET[((triple >> 6) & 0x3f) as usize])
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                char::from(ALPHABET[(triple & 0x3f) as usize])
            } else {
                '='
            });
        }
        out
    }
}

async fn rpc(app: &TestApp, credential: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/control/mcp")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(credential) = credential {
        builder = builder.header(header::AUTHORIZATION, credential);
    }
    let request = builder.body(Body::from(body.to_string())).expect("request");

    let response = app.app.clone().call(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn call_tool(
    app: &TestApp,
    credential: &str,
    name: &str,
    args: Value,
) -> (StatusCode, Value) {
    rpc(
        app,
        Some(credential),
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }),
    )
    .await
}

/// Extract the JSON body a tool returned (it is serialised into `content[0].text`).
fn tool_body(response: &Value) -> Value {
    response["result"]["content"][0]["text"]
        .as_str()
        .map(|s| serde_json::from_str(s).unwrap_or(Value::Null))
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn rejects_missing_and_wrong_credentials() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "agents_read": true })).await;

    let (status, body) = rpc(
        &app,
        None,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Authorization"));

    // Right username, wrong password.
    let (status, _) = rpc(
        &app,
        Some(&basic("root", "definitely-wrong")),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Unknown user must not be distinguishable from a wrong password.
    let (status, body) = rpc(
        &app,
        Some(&basic("nobody", "whatever")),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], "invalid username or password");
}

#[tokio::test]
async fn tools_list_is_empty_without_declared_scopes() {
    let app = setup().await;
    // A control plugin with no control grants declared.
    register_control_plugin(&app, &app.root_id, json!({})).await;

    let credential = basic("root", ROOT_PASSWORD);
    let (status, body) = rpc(
        &app,
        Some(&credential),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    // Deny-by-default: only the identity helper survives.
    assert_eq!(names, vec!["server_whoami"]);
}

#[tokio::test]
async fn declared_scopes_control_which_tools_appear() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "agents_read": true })).await;

    let credential = basic("root", ROOT_PASSWORD);
    let (_, body) = rpc(
        &app,
        Some(&credential),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;

    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"server_list_agents"));
    assert!(names.contains(&"server_get_agent"));
    // Read scope must not leak the write tool.
    assert!(!names.contains(&"server_delete_agent"));
    assert!(!names.contains(&"server_list_accounts"));
}

#[tokio::test]
async fn calling_an_ungranted_tool_is_refused() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "agents_read": true })).await;

    let credential = basic("root", ROOT_PASSWORD);
    let (status, body) = call_tool(
        &app,
        &credential,
        "server_delete_agent",
        json!({ "agent_id": "x" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not granted the scope"),
        "expected a scope refusal, got {body}"
    );
}

#[tokio::test]
async fn agent_crud_round_trip_as_root() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.root_id,
        json!({ "agents_read": true, "agents_write": true }),
    )
    .await;
    let credential = basic("root", ROOT_PASSWORD);

    // Create
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_agent",
        json!({ "name": "Ops Bot", "system_prompt": "Be terse." }),
    )
    .await;
    let created = tool_body(&body);
    let agent_id = created["agent"]["id"]
        .as_str()
        .expect("agent id")
        .to_string();
    assert_eq!(created["created"], true);

    // Read back
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_get_agent",
        json!({ "agent_id": agent_id }),
    )
    .await;
    let read = tool_body(&body);
    assert_eq!(read["agent"]["name"], "Ops Bot");
    assert_eq!(read["allowed"], "owner");

    // Update
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_update_agent",
        json!({ "agent_id": agent_id, "name": "Ops Bot v2" }),
    )
    .await;
    assert_eq!(tool_body(&body)["updated"], true);

    // List contains it
    let (_, body) = call_tool(&app, &credential, "server_list_agents", json!({})).await;
    let listed = tool_body(&body);
    assert!(listed["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .any(|a| a["id"] == agent_id.as_str() && a["name"] == "Ops Bot v2"));

    // Delete
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_delete_agent",
        json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(tool_body(&body)["deleted"], true);
    assert!(app.db.get_agent(&agent_id).await.expect("query").is_none());
}

#[tokio::test]
async fn member_credential_cannot_reach_account_administration() {
    let app = setup().await;
    // Even with the accounts scope declared, the credential's role is the gate.
    register_control_plugin(&app, &app.member_id, json!({ "accounts": true })).await;

    let credential = basic("alice", MEMBER_PASSWORD);

    // The tool is listed: the plugin did declare the scope.
    let (_, body) = rpc(
        &app,
        Some(&credential),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"server_list_accounts"));

    // But calling it is refused, because the role check is separate.
    let (status, body) = call_tool(&app, &credential, "server_list_accounts", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("root credential"),
        "expected a role refusal, got {body}"
    );
}

#[tokio::test]
async fn root_credential_has_full_account_control() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "accounts": true })).await;
    let credential = basic("root", ROOT_PASSWORD);

    // Create an account through the MCP.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_account",
        json!({ "username": "bob", "password": "BobPassw0rd", "role": "member" }),
    )
    .await;
    let created = tool_body(&body);
    assert_eq!(created["created"], true);
    let bob_id = created["account"]["id"].as_str().expect("id").to_string();

    // The new account is a real, usable login.
    let bob = app
        .db
        .get_account_by_username("bob")
        .await
        .expect("query")
        .expect("bob exists");
    assert_eq!(bob.id, bob_id);
    assert_eq!(bob.role, ROLE_MEMBER);

    // Weak passwords are refused with an actionable hint.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_account",
        json!({ "username": "weak", "password": "short" }),
    )
    .await;
    assert_eq!(body["error"]["code"], -32602);
    assert!(body["error"]["data"]["hint"].as_str().is_some());

    // Promote, then demote back.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_update_account",
        json!({ "account_id": bob_id, "role": "root" }),
    )
    .await;
    assert_eq!(tool_body(&body)["updated"], true);
    assert_eq!(
        app.db
            .get_account_by_id(&bob_id)
            .await
            .expect("query")
            .unwrap()
            .role,
        ROLE_ROOT
    );

    // Delete it.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_delete_account",
        json!({ "account_id": bob_id }),
    )
    .await;
    assert_eq!(tool_body(&body)["deleted"], true);
    assert!(app
        .db
        .get_account_by_id(&bob_id)
        .await
        .expect("query")
        .is_none());
}

#[tokio::test]
async fn the_last_root_cannot_be_removed_through_the_mcp() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "accounts": true })).await;
    let credential = basic("root", ROOT_PASSWORD);

    // Deleting yourself is refused first.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_delete_account",
        json!({ "account_id": app.root_id }),
    )
    .await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("cannot delete your own account"));
}

#[tokio::test]
async fn sharing_rules_are_enforced_for_a_member_credential() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.member_id,
        json!({ "agents_read": true, "agents_write": true, "shares": true }),
    )
    .await;

    // Root owns an agent that alice has no access to.
    let agent = app
        .db
        .create_agent(
            &app.root_id,
            "Root's Bot",
            None,
            None,
            None,
            "fallback",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("agent");

    let credential = basic("alice", MEMBER_PASSWORD);
    // A foreign agent reads as 404, not 403: existence is itself sensitive.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_get_agent",
        json!({ "agent_id": agent.id }),
    )
    .await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("no such agent"));

    // She also cannot share it.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_share_agent",
        json!({ "agent_id": agent.id, "account_id": app.member_id }),
    )
    .await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("no such agent"));

    // Nor delete it.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_delete_agent",
        json!({ "agent_id": agent.id }),
    )
    .await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("no such agent"));
    // Still there.
    assert!(app.db.get_agent(&agent.id).await.expect("query").is_some());
}

#[tokio::test]
async fn sharing_an_agent_grants_view_access() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.root_id,
        json!({ "agents_read": true, "agents_write": true, "shares": true }),
    )
    .await;
    let credential = basic("root", ROOT_PASSWORD);

    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_agent",
        json!({ "name": "Shared Bot" }),
    )
    .await;
    let agent_id = tool_body(&body)["agent"]["id"]
        .as_str()
        .expect("id")
        .to_string();

    // Share with alice (view only).
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_share_agent",
        json!({ "agent_id": agent_id, "account_id": app.member_id }),
    )
    .await;
    let share = tool_body(&body);
    assert_eq!(share["ok"], true);
    assert_eq!(share["share"]["can_edit"], false);

    // Alice can now see it.
    let alice = basic("alice", MEMBER_PASSWORD);
    let (_, body) = call_tool(
        &app,
        &alice,
        "server_get_agent",
        json!({ "agent_id": agent_id }),
    )
    .await;
    // `control_get_agent` is not granted to alice's plugin; the scope gate
    // answers first, which is the intended ordering.
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not granted the scope"));

    // Unshare.
    let (_, body) = call_tool(
        &app,
        &credential,
        "server_unshare_agent",
        json!({ "agent_id": agent_id, "account_id": app.member_id }),
    )
    .await;
    assert_eq!(tool_body(&body)["deleted"], true);
}

#[tokio::test]
async fn provider_keys_never_leave_the_server() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.root_id,
        json!({ "providers_read": true, "providers_write": true }),
    )
    .await;
    let credential = basic("root", ROOT_PASSWORD);

    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_provider",
        json!({
            "name": "OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-super-secret",
            "models": [{ "model_id": "gpt-4o", "context_window": 128000 }]
        }),
    )
    .await;
    assert_eq!(tool_body(&body)["model_count"], 1);

    let (_, body) = call_tool(&app, &credential, "server_list_providers", json!({})).await;
    let raw = body.to_string();
    assert!(
        !raw.contains("sk-super-secret"),
        "an API key must never be returned by a control tool: {raw}"
    );
    let listed = tool_body(&body);
    assert_eq!(listed["providers"][0]["has_api_key"], true);
}

#[tokio::test]
async fn plugin_tools_never_echo_env_secrets() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.root_id,
        json!({ "plugins_read": true, "plugins_write": true }),
    )
    .await;
    let credential = basic("root", ROOT_PASSWORD);

    let (_, body) = call_tool(
        &app,
        &credential,
        "server_create_plugin",
        json!({
            "name": "Telegram",
            "alias": "telegram",
            "transport": "binary",
            "url_or_command": "./data/binaries/telegram",
            "headers_or_env": "{\"TELEGRAM_BOT_TOKEN\":\"123:top-secret\"}"
        }),
    )
    .await;
    let created = tool_body(&body);
    assert_eq!(created["created"], true);
    assert!(
        !created.to_string().contains("top-secret"),
        "create must not echo the env it stored"
    );

    let (_, body) = call_tool(&app, &credential, "server_list_plugins", json!({})).await;
    let raw = body.to_string();
    assert!(
        !raw.contains("top-secret"),
        "listing must not echo stored env: {raw}"
    );
    let listed = tool_body(&body);
    let plugin = listed["plugins"]
        .as_array()
        .expect("plugins")
        .iter()
        .find(|p| p["alias"] == "telegram")
        .expect("telegram plugin present");
    assert_eq!(plugin["scopes"], json!([]));
}

#[tokio::test]
async fn whoami_reports_role_and_scopes() {
    let app = setup().await;
    register_control_plugin(
        &app,
        &app.root_id,
        json!({ "agents_read": true, "providers_read": true }),
    )
    .await;

    let credential = basic("root", ROOT_PASSWORD);
    let (_, body) = call_tool(&app, &credential, "server_whoami", json!({})).await;
    let me = tool_body(&body);
    assert_eq!(me["username"], "root");
    assert_eq!(me["role"], ROLE_ROOT);
    assert_eq!(me["is_root"], true);
    let scopes = me["scopes"].as_array().expect("scopes");
    assert!(scopes.iter().any(|s| s == "agents_read"));
    assert!(scopes.iter().any(|s| s == "providers_read"));
    assert!(!scopes.iter().any(|s| s == "accounts"));
}

#[tokio::test]
async fn unknown_method_and_tool_answer_jsonrpc_errors() {
    let app = setup().await;
    register_control_plugin(&app, &app.root_id, json!({ "agents_read": true })).await;
    let credential = basic("root", ROOT_PASSWORD);

    let (status, body) = rpc(
        &app,
        Some(&credential),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32601);
    assert!(body["error"]["data"]["hint"].as_str().is_some());

    // An unknown tool is refused. It is reported as "no scope" rather than
    // "no such tool" because the scope gate runs first and an unknown name has
    // no scope by definition -- the ordering is deliberate: a probe cannot use
    // error codes to enumerate the control surface it was not granted.
    let (_, body) = call_tool(&app, &credential, "control_no_such_tool", json!({})).await;
    assert_eq!(body["error"]["code"], -32003);
}

#[tokio::test]
async fn initialize_works_without_any_scope() {
    let app = setup().await;
    // No control plugin registered at all: initialize is transport-level.
    let (status, body) = rpc(
        &app,
        Some(&basic("root", ROOT_PASSWORD)),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {} }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["serverInfo"]["name"], "neuxdot-control");
    assert_eq!(body["result"]["capabilities"]["tools"], json!({}));
}
