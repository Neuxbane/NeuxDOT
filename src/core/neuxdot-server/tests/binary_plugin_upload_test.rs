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
        subscriptions: neuxdot_server::subscriptions::SubscriptionHub::new(
            db.clone(),
            event_tx.clone(),
        ),
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

// -----------------------------------------------------------------------------
// Re-uploading over a *running* binary: the ETXTBSY regression
// -----------------------------------------------------------------------------

/// Reproduce the exact failure from the UI: the target binary is currently
/// executing, so `fs::write` fails with `ETXTBSY` and the atomic replace must
/// succeed instead.
///
/// This is not a hypothetical: a plugin is bound to an agent, the agent's turn
/// has the binary mapped, and the operator uploads an update.
#[test]
fn uploading_over_a_running_binary_replaces_it_instead_of_failing() {
    use std::io::Read as _;

    let dir = std::env::temp_dir().join(format!("neuxdot-etxtbsy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");

    let target = dir.join("plugin");
    // ETXTBSY needs a *mapped executable*, not a shell script: `sh script` reads
    // the file, whereas an ELF binary stays mmap'd for its whole lifetime. Copy a
    // real binary and run it with a long sleep, exactly like a bound plugin.
    let source = ["/usr/bin/sleep", "/bin/sleep"]
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("need a sleep binary to reproduce ETXTBSY");
    std::fs::copy(source, &target).expect("copy real binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let mut child = std::process::Command::new(&target)
        .arg("5")
        .spawn()
        .expect("spawn the binary we are about to replace");

    // Give the kernel a moment to establish the executable mapping.
    std::thread::sleep(std::time::Duration::from_millis(200));

    // 1. The old approach must fail. If it does not, the test is not exercising
    //    the bug and its green result would be meaningless.
    let direct = std::fs::write(&target, b"\x7fELF replacement payload");
    assert!(
        direct.is_err(),
        "expected `fs::write` onto a running binary to fail with ETXTBSY; \
         this test no longer reproduces the reported bug"
    );

    // 2. The shipped fix must succeed.
    neuxdot_server::routes::mcp::write_binary_atomically(&target, b"\x7fELF replacement payload")
        .expect("atomic replace must succeed while the old binary runs");

    // 3. The bytes on disk are the new ones, and the mode survived.
    let mut content = Vec::new();
    std::fs::File::open(&target)
        .expect("open replaced binary")
        .read_to_end(&mut content)
        .expect("read");
    assert_eq!(content, b"\x7fELF replacement payload");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "replaced binary must stay executable");
    }

    // 4. No temporary files are left behind.
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn atomic_write_creates_missing_directories() {
    let dir = std::env::temp_dir().join(format!("neuxdot-mkdir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let target = dir.join("nested").join("bin").join("plugin");

    neuxdot_server::routes::mcp::write_binary_atomically(&target, b"content")
        .expect("creates parents and writes");
    assert_eq!(std::fs::read(&target).expect("read"), b"content");

    // Overwriting an existing, non-running file also works.
    neuxdot_server::routes::mcp::write_binary_atomically(&target, b"second").expect("overwrite");
    assert_eq!(std::fs::read(&target).expect("read"), b"second");

    let _ = std::fs::remove_dir_all(&dir);
}
