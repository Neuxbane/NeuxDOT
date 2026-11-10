//! The network allow-list an operator saves through the plugin API must be the
//! one the sandbox launch sees.
//!
//! This is the contract that failed in practice: the Telegram plugin kept
//! spawning with `no network` after the allow-list was saved, so every poll
//! died with a bare reqwest error. The chain tested here is
//! API create/update -> `permissions_json` -> `get_permissions()` ->
//! `SandboxSpec::launch_for` -> `uses_proxy()`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use neuxdot_core::auth::create_token;
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::mcp::sandbox::{Launch, SandboxSpec};
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_service::Service;

async fn app() -> (axum::Router, Arc<Database>, String) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (root, _) = db.get_or_init_root_account().await.unwrap();
    let token = create_token(&root.id, &root.username, &root.role).unwrap();
    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    let router = build_router(AppState {
        subscriptions: neuxdot_server::subscriptions::SubscriptionHub::new(
            Arc::clone(&db),
            event_tx.clone(),
        ),
        db: Arc::clone(&db),
        event_tx,
    });
    (router, db, token)
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router.clone().call(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn stored_network_allow_list_reaches_the_sandbox_launch() {
    let (router, db, token) = app().await;

    let perms = json!({
        "ui": { "enabled": false, "resources": [] },
        "storage": { "get": false, "set": false, "delete": false, "keys": [] },
        "agent": { "trigger": true, "tools": true },
        "identity": { "agent.name": false, "agent.profile_picture": false,
                      "user.name": false, "user.profile_picture": false },
        "process": true,
        "network": { "allow": ["api.telegram.org"] },
        "elevated": false,
        "control": { "agents_read": false, "agents_write": false, "shares": false,
                     "plugins_read": false, "plugins_write": false,
                     "providers_read": false, "providers_write": false,
                     "accounts": false }
    });

    let (status, created) = call(
        &router,
        "POST",
        "/api/mcp",
        &token,
        Some(json!({
            "name": "Telegram",
            "alias": "telegram",
            "transport": "binary",
            "url_or_command": "./release/plugins/telegram",
            "permissions": perms
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    let id = created["id"].as_str().unwrap().to_string();

    let server = db.get_mcp_server(&id).await.unwrap().expect("row");
    let resolved = server.get_permissions();
    assert!(
        resolved.network.allows_host("api.telegram.org"),
        "stored consent lost the network grant: {:?}",
        resolved.network.allow
    );

    let launch = SandboxSpec::launch_for(&server);
    match launch {
        Launch::Sandboxed(spec) => {
            assert!(spec.uses_proxy(), "allow-list must launch with the proxy");
            assert!(!spec.shares_host_network(), "allow-list must not share host net");
        }
        Launch::Elevated => panic!("a network allow-list must stay sandboxed"),
    }
}

#[tokio::test]
async fn update_network_allow_list_reaches_the_sandbox_launch() {
    let (router, db, token) = app().await;

    let (_, created) = call(
        &router,
        "POST",
        "/api/mcp",
        &token,
        Some(json!({
            "name": "Telegram",
            "transport": "binary",
            "url_or_command": "./release/plugins/telegram"
        })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    // Initially denied.
    let server = db.get_mcp_server(&id).await.unwrap().unwrap();
    assert!(server.get_permissions().network.is_denied());

    // Save an allow-list the way the /mcp form does.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/api/mcp/{id}"),
        &token,
        Some(json!({
            "permissions": {
                "ui": { "enabled": false, "resources": [] },
                "storage": { "get": false, "set": false, "delete": false, "keys": [] },
                "agent": { "trigger": true, "tools": true },
                "identity": { "agent.name": false, "agent.profile_picture": false,
                              "user.name": false, "user.profile_picture": false },
                "process": true,
                "network": { "allow": ["api.telegram.org"] },
                "elevated": false,
                "control": { "agents_read": false, "agents_write": false, "shares": false,
                             "plugins_read": false, "plugins_write": false,
                             "providers_read": false, "providers_write": false,
                             "accounts": false }
            }
        })),
    )
    .await;
    assert!(status.is_success(), "update failed: {updated}");

    let server = db.get_mcp_server(&id).await.unwrap().unwrap();
    assert!(
        server.get_permissions().network.allows_host("api.telegram.org"),
        "update did not persist the network grant"
    );

    match SandboxSpec::launch_for(&server) {
        Launch::Sandboxed(spec) => assert!(spec.uses_proxy()),
        Launch::Elevated => panic!("must stay sandboxed"),
    }
}
