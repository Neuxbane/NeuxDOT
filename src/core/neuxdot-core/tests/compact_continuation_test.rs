//! `compact` must accept the model's own handoff instruction and feed it back
//! as the rebuilt session's first input.
//!
//! Without it, compaction rebuilt the context and then waited for the next
//! event, even though the model knew exactly what it was in the middle of.
//! `compact({ continuation: "…" })` now persists that instruction as a `user`
//! row and the turn loop immediately asks the model again, so the rebuilt
//! session starts working on it with no external wake-up.

use std::sync::{Arc, Mutex};

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ActiveModelTarget;
use neuxdot_core::runtime::executor::AgentExecutor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CONTINUATION: &str = "Continue: finish indexing the repository, then report the count.";

/// Mock OpenAI-compatible server. First completion asks for `compact` with a
/// continuation, second answers DONE. Captures every request body so we can
/// prove the continuation was sent back to the model.
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
            let mut buf = vec![0u8; 131_072];
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
                                "id": "call_compact",
                                "type": "function",
                                "function": {
                                    "name": "compact",
                                    "arguments": json!({ "continuation": CONTINUATION }).to_string()
                                }
                            }]
                        }
                    }]
                })
            } else {
                json!({
                    "id": "chatcmpl-test",
                    "choices": [{
                        "message": { "role": "assistant", "content": "DONE" }
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
async fn compact_continuation_becomes_the_rebuilt_sessions_first_input() {
    let (base_url, bodies) = mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Handoff",
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

    let result = executor.run_turn("start").await;
    assert_eq!(result.as_deref(), Ok("DONE"));

    // The model was invoked again without any external event, and the second
    // request carries the handoff as a user message.
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "compaction must not end the turn");
    assert!(
        bodies[1].contains(CONTINUATION),
        "second completion must include the continuation: {}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("\"role\":\"user\""),
        "the continuation must be a user input, not a system note"
    );

    // It is persisted too, so a restart resumes from the same instruction.
    let history = db.get_agent_history(&agent.id).await.unwrap();
    let handoff = history
        .iter()
        .find(|h| h.role == "user" && h.content == CONTINUATION)
        .expect("the continuation must be persisted as a user row");
    assert_eq!(handoff.role, "user");
    // The final assistant answer follows it, proving the model acted on it.
    assert_eq!(history.last().unwrap().role, "assistant");
}
