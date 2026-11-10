//! HTTP-level tests for per-agent MCP plugin configuration with global fallback.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use neuxdot_core::auth::{create_token, hash_password};
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_service::Service;

struct TestApp {
    app: Router,
    db: Arc<Database>,
    token_a: String,
    token_b: String,
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {}", token));
    }
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let response = app.clone().call(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

async fn setup() -> TestApp {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));

    let (_root, _) = db.get_or_init_root_account().await.unwrap();

    let hash = hash_password("SomePassw0rd").unwrap();
    let a = db.create_account("alice", &hash).await.unwrap();
    let b = db.create_account("bob", &hash).await.unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    let app = build_router(AppState {
        subscriptions: neuxdot_server::subscriptions::SubscriptionHub::new(
            Arc::clone(&db).clone(),
            event_tx.clone(),
        ),
        db: Arc::clone(&db),
        event_tx,
    });

    TestApp {
        app,
        db,
        token_a: create_token(&a.id, "alice", "member").unwrap(),
        token_b: create_token(&b.id, "bob", "member").unwrap(),
    }
}

#[tokio::test]
async fn test_per_agent_mcp_config_override_and_fallback() {
    let t = setup().await;

    // 1. Alice creates an agent
    let (st, res) = request(
        &t.app,
        "POST",
        "/api/agents",
        &t.token_a,
        Some(json!({ "name": "Agent Alice" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let agent_id = res["id"].as_str().unwrap().to_string();

    // 2. Alice creates a global plugin with a default token
    let global_config = json!({
        "API_TOKEN": "global_shared_secret",
        "BASE_URL": "https://api.example.com"
    })
    .to_string();

    let (st, plugin_res) = request(
        &t.app,
        "POST",
        "/api/mcp",
        &t.token_a,
        Some(json!({
            "name": "GitService",
            "transport": "stdio",
            "url_or_command": "echo",
            "headers_or_env": global_config
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let plugin_id = plugin_res["id"].as_str().unwrap().to_string();

    // Attach plugin to agent
    let _ =
        t.db.set_agent_mcp_servers(&agent_id, &[plugin_id.clone()])
            .await;

    // 3. Query agent plugin config: Initially no override, effective is global default
    let (st, cfg_res) = request(
        &t.app,
        "GET",
        &format!("/api/agents/{}/mcp/{}/config", agent_id, plugin_id),
        &t.token_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cfg_res["has_override"], false);
    assert_eq!(cfg_res["config_override"], Value::Null);
    assert!(cfg_res["effective_config"]
        .as_str()
        .unwrap()
        .contains("global_shared_secret"));

    // 4. Set Agent override with a specific token
    let override_json = json!({ "API_TOKEN": "agent_private_secret" }).to_string();
    let (st, put_res) = request(
        &t.app,
        "PUT",
        &format!("/api/agents/{}/mcp/{}/config", agent_id, plugin_id),
        &t.token_a,
        Some(json!({ "config_override": override_json })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(put_res["success"], true);

    // 5. Query again: effective config now reflects the agent override for API_TOKEN while keeping BASE_URL
    let (st, cfg_res2) = request(
        &t.app,
        "GET",
        &format!("/api/agents/{}/mcp/{}/config", agent_id, plugin_id),
        &t.token_a,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cfg_res2["has_override"], true);
    let eff = cfg_res2["effective_config"].as_str().unwrap();
    assert!(eff.contains("agent_private_secret"));
    assert!(eff.contains("https://api.example.com"));

    // 6. Verify via db.get_agent_mcp_servers that runtime receives effective config
    let servers = t.db.get_agent_mcp_servers(&agent_id).await.unwrap();
    assert_eq!(servers.len(), 1);
    let srv_headers = servers[0].headers_or_env.as_deref().unwrap();
    assert!(srv_headers.contains("agent_private_secret"));
    assert!(srv_headers.contains("https://api.example.com"));

    // 7. Security: Bob cannot read or modify Alice's agent's plugin config
    let (st, _) = request(
        &t.app,
        "GET",
        &format!("/api/agents/{}/mcp/{}/config", agent_id, plugin_id),
        &t.token_b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _) = request(
        &t.app,
        "PUT",
        &format!("/api/agents/{}/mcp/{}/config", agent_id, plugin_id),
        &t.token_b,
        Some(json!({ "config_override": "hacked" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}
