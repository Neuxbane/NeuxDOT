//! Test multipart upload of binary MCP server plugins and saving to data/binaries/.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use neuxdot_core::auth::create_token;
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::Value;
use std::sync::Arc;
use tower_service::Service;

#[tokio::test]
async fn test_upload_binary_handler() {
    let pool = init_db("sqlite::memory:").await.expect("init db");
    let db = Arc::new(Database::new(pool));
    let (root, _) = db.get_or_init_root_account().await.expect("root");
    let token = create_token(&root.id, &root.username, &root.role).expect("token");

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let state = AppState {
        db: db.clone(),
        event_tx,
    };
    let mut app = build_router(state);

    let boundary = "---------------------------1234567890123456789012345678";
    let body_data = format!(
        "--{boundary}\r\n\
        Content-Disposition: form-data; name=\"file\"; filename=\"my-mcp-tool\"\r\n\
        Content-Type: application/octet-stream\r\n\r\n\
        #!/bin/sh\necho '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}'\r\n\
        --{boundary}--\r\n"
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins/upload-binary")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_data))
        .unwrap();

    let resp = app.call(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(json["filename"], "my-mcp-tool");
    assert_eq!(json["path"], "./data/binaries/my-mcp-tool");
    assert_eq!(json["suggested_alias"], "my_mcp_tool");
    assert_eq!(json["suggested_name"], "My Mcp Tool");

    assert!(std::path::Path::new("data/binaries/my-mcp-tool").exists());
    let _ = std::fs::remove_file("data/binaries/my-mcp-tool");
}
