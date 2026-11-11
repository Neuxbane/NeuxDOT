//! `imageView` must turn a text-only image payload into a real image part.
//!
//! Tool results are strings: a plugin returning base64 gets truncated and the
//! model sees `iVBORw0KGgo…` forever. The model now calls
//! `imageView({ source })`; the host stores the decoded bytes and injects an
//! `image_url` content part, so the next completion actually sees the picture.

use std::sync::{Arc, Mutex};

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ActiveModelTarget;
use neuxdot_core::runtime::executor::AgentExecutor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 1x1 transparent PNG.
const PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

async fn mock_provider() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&bodies);

    tokio::spawn(async move {
        let mut round = 0usize;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 524_288];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                if let Some((_, body)) = text.split_once("\r\n\r\n") {
                    captured.lock().unwrap().push(body.to_string());
                }
            }

            let body = if round == 0 {
                json!({
                    "id": "chatcmpl-test",
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_image",
                                "type": "function",
                                "function": {
                                    "name": "imageView",
                                    "arguments": json!({
                                        "source": format!("data:image/png;base64,{PNG_B64}"),
                                        "detail": "high"
                                    })
                                    .to_string()
                                }
                            }]
                        }
                    }]
                })
            } else {
                json!({
                    "id": "chatcmpl-test",
                    "choices": [{
                        "message": { "role": "assistant", "content": "SEEN" }
                    }]
                })
            }
            .to_string();
            round += 1;

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });

    (format!("http://{addr}"), bodies)
}

fn target(base_url: String) -> ActiveModelTarget {
    ActiveModelTarget {
        id: "pm_test".into(),
        provider_id: "p_test".into(),
        provider_name: "Mock".into(),
        base_url,
        api_key: Some("test-key".into()),
        model_id: "mock-model".into(),
        model_name: Some("Mock Model".into()),
        config_json: None,
        params_json: None,
        context_window: Some(128_000),
    }
}

#[tokio::test]
async fn image_view_attaches_a_real_image_part_to_the_next_completion() {
    let (base_url, bodies) = mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Eyes",
            None,
            None,
            None,
            "fallback",
            Some("You are {{agent.name}}."),
            Some(true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(256);
    let executor = AgentExecutor::new(
        Arc::clone(&db),
        agent.clone(),
        vec![target(base_url)],
        Vec::new(),
        event_tx,
    );

    let result = executor.run_turn("look at this").await;
    assert_eq!(result.as_deref(), Ok("SEEN"));

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[1].contains("\"image_url\""),
        "the image must be a real content part: {}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("data:image/png;base64,"),
        "the stored image must be expanded to a data URL"
    );
    assert!(
        bodies[1].contains("\"detail\":\"high\""),
        "the model's detail hint must be preserved"
    );

    // History keeps only the compact reference, never the base64 itself.
    let history = db.get_agent_history(&agent.id).await.unwrap();
    let media_row = history
        .iter()
        .find(|h| h.role == "user" && h.content.contains("\"type\":\"image\""))
        .expect("the image reference must be persisted");
    assert!(!media_row.content.contains(PNG_B64));
    let path: serde_json::Value = serde_json::from_str(&media_row.content).unwrap();
    let stored = path[1]["path"].as_str().unwrap();
    assert!(
        std::path::Path::new(stored).exists(),
        "the decoded image must be stored on disk at {stored}"
    );
}
