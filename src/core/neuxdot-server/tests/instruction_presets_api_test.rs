//! HTTP-level tests for per-account Agent Instructions presets.
//!
//! The router, SQLite and JWTs are real, so this covers route wiring, owner
//! scoping and the 400/404 semantics.

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
    token_a: String,
    token_b: String,
}

async fn request(app: &Router, method: &str, uri: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
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
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
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

    // Two independent accounts -> presets must not leak between them.
    let hash = hash_password("SomePassw0rd").unwrap();
    let a = db.create_account("alice", &hash).await.unwrap();
    let b = db.create_account("bob", &hash).await.unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    let app = build_router(AppState { db, event_tx });

    TestApp {
        app,
        token_a: create_token(&a.id, &a.username, &a.role).unwrap(),
        token_b: create_token(&b.id, &b.username, &b.role).unwrap(),
    }
}

#[tokio::test]
async fn preset_crud_roundtrip() {
    let t = setup().await;

    // Empty list initially.
    let (status, body) = request(&t.app, "GET", "/api/instruction-presets", &t.token_a, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);

    // Create.
    let (status, created) = request(
        &t.app,
        "POST",
        "/api/instruction-presets",
        &t.token_a,
        Some(json!({ "name": "Coder", "description": "Rust helper", "content": "You are {{agent.name}}." })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {:?}", created);
    assert_eq!(created["name"], "Coder");
    let id = created["id"].as_str().unwrap().to_string();

    // Listed.
    let (_, list) = request(&t.app, "GET", "/api/instruction-presets", &t.token_a, None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Update.
    let (status, updated) = request(
        &t.app,
        "PATCH",
        &format!("/api/instruction-presets/{}", id),
        &t.token_a,
        Some(json!({ "name": "Coder v2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "patch: {:?}", updated);
    assert_eq!(updated["name"], "Coder v2");
    // Untouched fields survive a partial update.
    assert_eq!(updated["content"], "You are {{agent.name}}.");

    // Delete.
    let (status, _) = request(&t.app, "DELETE", &format!("/api/instruction-presets/{}", id), &t.token_a, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = request(&t.app, "GET", "/api/instruction-presets", &t.token_a, None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn presets_are_isolated_per_account() {
    let t = setup().await;

    let (_, created) = request(
        &t.app,
        "POST",
        "/api/instruction-presets",
        &t.token_a,
        Some(json!({ "name": "Alice only", "content": "secret instructions" })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    // B must not see it.
    let (status, list) = request(&t.app, "GET", "/api/instruction-presets", &t.token_b, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 0, "B must not see A's presets");

    // B cannot read, update or delete it (404, not 403, to avoid leaking existence).
    let (status, _) = request(&t.app, "PATCH", &format!("/api/instruction-presets/{}", id), &t.token_b,
        Some(json!({ "name": "hijacked" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(&t.app, "DELETE", &format!("/api/instruction-presets/{}", id), &t.token_b, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Still intact and unchanged for A.
    let (_, list) = request(&t.app, "GET", "/api/instruction-presets", &t.token_a, None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "Alice only");
}

#[tokio::test]
async fn invalid_presets_are_rejected() {
    let t = setup().await;

    let (status, _) = request(&t.app, "POST", "/api/instruction-presets", &t.token_a,
        Some(json!({ "name": "   ", "content": "x" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "blank name");

    let (status, _) = request(&t.app, "POST", "/api/instruction-presets", &t.token_a,
        Some(json!({ "name": "ok", "content": "  " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "blank content");

    let (status, _) = request(&t.app, "PATCH", "/api/instruction-presets/does-not-exist", &t.token_a,
        Some(json!({ "name": "x" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
