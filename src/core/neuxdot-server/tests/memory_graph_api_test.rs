//! HTTP surface for the agent memory graph (`GET /api/agents/:id/memory/graph`).
//!
//! The graph is what the UI renders; it must expose wikilink edges (including
//! targets that have no node yet) and must not leak a foreign agent's memory.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use neuxdot_core::auth::{create_token, hash_password};
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::Account;
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::Value;
use std::sync::Arc;
use tower_service::Service;

struct TestApp {
    app: Router,
    db: Arc<Database>,
    alice: Account,
    token_a: String,
    token_b: String,
}

async fn request(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {}", token));
    let response = app
        .clone()
        .call(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
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
    let alice = db.create_account("alice", &hash).await.unwrap();
    let bob = db.create_account("bob", &hash).await.unwrap();

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
        token_a: create_token(&alice.id, "alice", "member").unwrap(),
        token_b: create_token(&bob.id, "bob", "member").unwrap(),
        alice,
    }
}

#[tokio::test]
async fn graph_returns_nodes_with_wikilink_edges_including_dangling_targets() {
    let t = setup().await;
    let agent =
        t.db.create_agent(
            &t.alice.id,
            "A",
            None,
            None,
            None,
            "fallback",
            None,
            Some(true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    t.db.upsert_memory_node(
        &agent.id,
        "deepseek v4",
        "Vendor [[kenari]] ships it.",
        "entity",
        "The model",
        "h1",
    )
    .await
    .unwrap();

    let (status, body) = request(
        &t.app,
        &format!("/api/agents/{}/memory/graph", agent.id),
        &t.token_a,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["key"], "deepseek v4");
    assert_eq!(nodes[0]["type"], "entity");
    // `kenari` has no node: the edge is still reported, so the client can draw a
    // dangling target instead of silently dropping the link.
    assert_eq!(nodes[0]["outbound"][0], "kenari");
    assert!(nodes[0]["backlinks"].as_array().unwrap().is_empty());
}

/// A foreign agent is refused before any node is read. The status matches every
/// other agent-scoped route (`load_agent_with_access` reports 403 for an agent
/// that exists but is not accessible), so the graph does not become an oracle
/// that behaves differently from the rest of the API.
#[tokio::test]
async fn graph_is_forbidden_for_a_foreign_agent() {
    let t = setup().await;
    let agent =
        t.db.create_agent(
            &t.alice.id,
            "A",
            None,
            None,
            None,
            "fallback",
            None,
            Some(true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let (status, _) = request(
        &t.app,
        &format!("/api/agents/{}/memory/graph", agent.id),
        &t.token_b,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
