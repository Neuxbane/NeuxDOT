//! HTTP-level tests for sharing an agent between accounts.

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

struct App {
    router: Router,
    owner_token: String,
    friend_token: String,
    owner_id: String,
    friend_id: String,
    agent_id: String,
}

async fn call(r: &Router, method: &str, uri: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        b = b.header("authorization", format!("Bearer {}", token));
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = r.clone().call(b.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, v)
}

async fn setup() -> App {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let hash = hash_password("SomePassw0rd").unwrap();
    let owner = db.create_account("owner", &hash).await.unwrap();
    let friend = db.create_account("friend", &hash).await.unwrap();

    let owner_token = create_token(&owner.id, &owner.username, &owner.role).unwrap();
    let friend_token = create_token(&friend.id, &friend.username, &friend.role).unwrap();

    let agent = db
        .create_agent(&owner.id, "SharedBot", None, None, None, "fallback", Some("sys"), None, None, None, None, None)
        .await
        .unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    App {
        router: build_router(AppState { db, event_tx }),
        owner_token,
        friend_token,
        owner_id: owner.id,
        friend_id: friend.id,
        agent_id: agent.id,
    }
}

#[tokio::test]
async fn share_then_visible_in_friend_list() {
    let t = setup().await;

    // Not shared yet: the friend cannot see it at all.
    let (s, _) = call(&t.router, "GET", &format!("/api/agents/{}", t.agent_id), &t.friend_token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, share) = call(
        &t.router,
        "POST",
        &format!("/api/agents/{}/shares", t.agent_id),
        &t.owner_token,
        Some(json!({ "username": "friend", "can_edit": false })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create share: {:?}", share);

    // Now it appears in the friend's list, flagged as not owned.
    let (s, list) = call(&t.router, "GET", "/api/agents", &t.friend_token, None).await;
    assert_eq!(s, StatusCode::OK);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "friend list: {:?}", list);
    assert_eq!(arr[0]["is_owner"], false);
    assert_eq!(arr[0]["can_edit"], false);

    // Owner's own list still shows it as owned.
    let (_, list) = call(&t.router, "GET", "/api/agents", &t.owner_token, None).await;
    assert_eq!(list[0]["is_owner"], true);
}

#[tokio::test]
async fn sharee_cannot_delete_and_cannot_edit_without_permission() {
    let t = setup().await;
    call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "friend" }))).await;

    // Delete is owner-only.
    let (s, body) = call(&t.router, "DELETE", &format!("/api/agents/{}", t.agent_id), &t.friend_token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "sharee delete: {:?}", body);

    // No can_edit -> PATCH refused.
    let (s, _) = call(&t.router, "PATCH", &format!("/api/agents/{}", t.agent_id), &t.friend_token,
        Some(json!({ "name": "Hacked" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Grant edit -> PATCH allowed.
    let (s, _) = call(&t.router, "PATCH", &format!("/api/agents/{}/shares/{}", t.agent_id, t.friend_id),
        &t.owner_token, Some(json!({ "can_edit": true }))).await;
    assert_eq!(s, StatusCode::OK);

    let (s, updated) = call(&t.router, "PATCH", &format!("/api/agents/{}", t.agent_id), &t.friend_token,
        Some(json!({ "name": "Renamed" }))).await;
    assert_eq!(s, StatusCode::OK, "patch with can_edit: {:?}", updated);

    // Still cannot delete, even with edit rights.
    let (s, _) = call(&t.router, "DELETE", &format!("/api/agents/{}", t.agent_id), &t.friend_token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sharee_cannot_manage_shares_without_permission_but_can_leave() {
    let t = setup().await;
    let hash = hash_password("SomePassw0rd").unwrap();
    // third account to attempt to add
    let (s, _) = call(&t.router, "POST", "/api/accounts", &t.owner_token,
        Some(json!({ "username": "third", "password": "ThirdPass123" }))).await;
    let _ = (s, hash);

    call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "friend" }))).await;

    // Friend cannot add someone else.
    let (s, _) = call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.friend_token,
        Some(json!({ "username": "third", "can_edit": true }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // But can always remove their own access.
    let (s, _) = call(&t.router, "DELETE", &format!("/api/agents/{}/shares/{}", t.agent_id, t.friend_id),
        &t.friend_token, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Access is gone.
    let (s, _) = call(&t.router, "GET", &format!("/api/agents/{}", t.agent_id), &t.friend_token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn share_validation_and_owner_protection() {
    let t = setup().await;

    // Unknown username.
    let (s, _) = call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "ghost" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Sharing with the owner is pointless.
    let (s, _) = call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "owner" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Duplicate share.
    call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "friend" }))).await;
    let (s, _) = call(&t.router, "POST", &format!("/api/agents/{}/shares", t.agent_id), &t.owner_token,
        Some(json!({ "username": "friend" }))).await;
    assert_eq!(s, StatusCode::CONFLICT);

    // The owner's share cannot be removed.
    let (s, _) = call(&t.router, "DELETE", &format!("/api/agents/{}/shares/{}", t.agent_id, t.owner_id),
        &t.owner_token, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
