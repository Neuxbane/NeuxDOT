//! Only one turn may run per agent, and mid-turn arrivals are merged into the
//! next batch.
//!
//! The bug this pins: the HTTP route, the MCP push bridge and the subscription
//! hub could each spawn a `run_turn`. Two concurrent turns both called `sleep`,
//! and each `register_timer` cancelled the other, producing the endless
//! `SLEEP 30m -> WAKE cancelled: superseded by new timer` ping-pong.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ActiveModelTarget;
use neuxdot_core::runtime::executor::AgentExecutor;
use neuxdot_core::runtime::turn_queue::global_turn_queue;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Mock provider that records request bodies and takes a moment to answer, so
/// concurrent submissions are guaranteed to overlap the first turn.
async fn slow_mock_provider() -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bodies);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1 << 20];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(body) = raw.split("\r\n\r\n").nth(1) {
                    if let Ok(v) = serde_json::from_str::<Value>(body) {
                        sink.lock().unwrap().push(v);
                    }
                }
                // Long enough that later submissions land while this turn runs.
                tokio::time::sleep(Duration::from_millis(300)).await;
                let body = json!({
                    "id": "chatcmpl-test",
                    "choices": [{ "message": { "role": "assistant", "content": "ok" } }]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://{addr}"), bodies)
}


fn make_executor(
    db: &Arc<Database>,
    agent: &neuxdot_core::db::schema::Agent,
    base_url: &str,
    event_tx: &tokio::sync::broadcast::Sender<neuxdot_core::events::EventEnvelope>,
) -> AgentExecutor {
    AgentExecutor::new(
        Arc::clone(db),
        agent.clone(),
        vec![target(base_url.to_string())],
        Vec::new(),
        event_tx.clone(),
    )
}

fn target(base_url: String) -> ActiveModelTarget {
    ActiveModelTarget {
        id: "pm_test".into(),
        provider_id: "p_test".into(),
        provider_name: "Mock".into(),
        base_url,
        api_key: Some("k".into()),
        model_id: "mock".into(),
        model_name: None,
        config_json: None,
        params_json: None,
        context_window: Some(128_000),
    }
}

#[tokio::test]
async fn mid_turn_arrivals_batch_into_one_follow_up_turn() {
    let (base_url, bodies) = slow_mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Batcher",
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

    // Start the first turn, then two more while it is still in flight.
    let first = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("first").await }
    });
    tokio::time::sleep(Duration::from_millis(80)).await;

    let second = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("second").await }
    });
    let third = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("third").await }
    });

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    third.await.unwrap().unwrap();

    let captured = bodies.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        2,
        "expected one turn for the first message and one batched turn, got {}",
        captured.len()
    );

    let second_prompt = captured[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .last()
        .unwrap_or_default()
        .to_string();
    assert!(
        second_prompt.contains("2 messages arrived"),
        "the follow-up turn must carry a merged batch, got: {second_prompt}"
    );
    assert!(second_prompt.contains("second") && second_prompt.contains("third"));

    assert!(
        !global_turn_queue().is_running(&agent.id),
        "the agent must be released once the queue is drained"
    );
}

#[tokio::test]
async fn a_single_message_queued_mid_turn_is_not_wrapped() {
    let (base_url, bodies) = slow_mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Single",
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

    let first = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("first").await }
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    let second = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("second").await }
    });

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();

    let captured = bodies.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    // The last user message is the newly submitted one; earlier turns remain in
    // history by design.
    let second_prompt = captured[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .last()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        second_prompt, "second",
        "a lone queued message must pass through unwrapped"
    );
}
