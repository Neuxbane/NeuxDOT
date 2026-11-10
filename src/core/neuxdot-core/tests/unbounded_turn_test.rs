//! A turn must not be capped at a fixed number of iterations.
//!
//! The executor used to run `for _ in 0..8`, so a long-horizon agent was
//! silently cut off mid-work and the turn reported success with partial output.
//! This test drives a real `run_turn` against a mock provider that asks for a
//! tool call 12 times before answering. With the old cap the turn stops at 8;
//! with an unbounded loop it reaches the final answer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ActiveModelTarget;
use neuxdot_core::runtime::executor::AgentExecutor;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOOL_ROUNDS: usize = 12;

/// Minimal OpenAI-compatible server: `TOOL_ROUNDS` tool-call replies, then a
/// final answer. Counts how many completions were requested.
async fn mock_provider() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            // Drain the request head + body so the client sees a clean close.
            let mut buf = vec![0u8; 65536];
            let _ = socket.read(&mut buf).await;

            let body = if n < TOOL_ROUNDS {
                json!({
                    "id": "chatcmpl-test",
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": format!("call_{n}"),
                                "type": "function",
                                "function": { "name": "list_servers", "arguments": "{}" }
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

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });

    (format!("http://{addr}"), calls)
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
async fn a_turn_runs_past_eight_iterations_until_the_agent_is_done() {
    let (base_url, calls) = mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Long Horizon",
            None,
            None,
            None,
            "fallback",
            Some("You are {{agent.name}}."),
            Some(true), // output text allowed -> a final answer ends the turn
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
        agent,
        vec![target(base_url)],
        Vec::new(),
        event_tx,
    );

    let result = executor.run_turn("start").await;

    assert_eq!(
        result.as_deref(),
        Ok("DONE"),
        "the turn must reach the model's final answer, not stop at an iteration cap"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        TOOL_ROUNDS + 1,
        "expected {TOOL_ROUNDS} tool rounds + 1 final completion"
    );
}
