//! `sleep` is the turn boundary that lets a queued batch through.
//!
//! A tool-only agent's turn is unbounded, so "wait for the turn to finish" is
//! not enough: an agent that just sleeps forever would starve every queued
//! message. `sleep` is the agent declaring it has nothing to do, so once a
//! sleep completes and work is waiting, the turn ends and the batch runs next.
//!
//! The running turn is never interrupted: it sleeps for its full duration and
//! then yields. This test proves both halves — the sleep was not cut short, and
//! the queued message was handled afterwards in a second turn.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::{ActiveModelTarget, Agent};
use neuxdot_core::events::EventEnvelope;
use neuxdot_core::runtime::executor::AgentExecutor;
use neuxdot_core::runtime::turn_queue::global_turn_queue;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SLEEP_SECS: u64 = 2;

/// First completion asks for `sleep(2s)`; every later one answers with text.
async fn mock_provider(
    sleep_args: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let count = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&bodies);
    let counter = Arc::clone(&count);
    let sleep_args = Arc::new(sleep_args);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            let counter = Arc::clone(&counter);
            let sleep_args = Arc::clone(&sleep_args);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1 << 20];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(body) = raw.split("\r\n\r\n").nth(1) {
                    if let Ok(v) = serde_json::from_str::<Value>(body) {
                        sink.lock().unwrap().push(v);
                    }
                }
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let body = if n == 0 {
                    json!({
                        "id": "c",
                        "choices": [{ "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_sleep",
                                "type": "function",
                                "function": { "name": "sleep",
                                              "arguments": sleep_args.to_string() }
                            }]
                        }}]
                    })
                } else {
                    json!({
                        "id": "c",
                        "choices": [{ "message": { "role": "assistant", "content": "ok" } }]
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://{addr}"), bodies, count)
}

fn target(base_url: String) -> ActiveModelTarget {
    ActiveModelTarget {
        id: "pm".into(),
        provider_id: "p".into(),
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

fn make_executor(
    db: &Arc<Database>,
    agent: &Agent,
    base_url: &str,
    event_tx: &tokio::sync::broadcast::Sender<EventEnvelope>,
) -> AgentExecutor {
    AgentExecutor::new(
        Arc::clone(db),
        agent.clone(),
        vec![target(base_url.to_string())],
        Vec::new(),
        event_tx.clone(),
    )
}

async fn run_sleeper(sleep_args: Value) -> (Duration, usize, String) {
    let (base_url, bodies, count) = mock_provider(sleep_args).await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Sleeper",
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

    let started = Instant::now();
    let sleeper = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("first").await }
    });

    // Arrive while the agent is asleep. This must be queued, not injected.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let queued = tokio::spawn({
        let db = Arc::clone(&db);
        let agent = agent.clone();
        let base_url = base_url.clone();
        let event_tx = event_tx.clone();
        async move { make_executor(&db, &agent, &base_url, &event_tx).run_turn("second").await }
    });

    sleeper.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();

    let elapsed = started.elapsed();
    let calls = count.load(Ordering::SeqCst);
    let second_prompt = bodies.lock().unwrap()[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .last()
        .unwrap_or_default()
        .to_string();

    assert!(!global_turn_queue().is_running(&agent.id));
    (elapsed, calls, second_prompt)
}

/// `interruptible: false` must be honoured: the message waits for the timer.
#[tokio::test]
async fn non_interruptible_sleep_is_not_cut_short() {
    let (elapsed, calls, second_prompt) = run_sleeper(json!({
        "duration": format!("{SLEEP_SECS}s"),
        "interruptible": false
    }))
    .await;

    assert_eq!(calls, 2, "one completion for the sleep, one for the batch");
    assert!(
        elapsed >= Duration::from_secs(SLEEP_SECS),
        "a non-interruptible sleep must run its full duration; elapsed {elapsed:?}"
    );
    assert_eq!(second_prompt, "second");
}

/// The default is interruptible, so an arriving message wakes the agent early
/// and becomes the next turn.
#[tokio::test]
async fn default_sleep_is_interrupted_by_a_queued_message() {
    let (elapsed, calls, second_prompt) = run_sleeper(json!({
        "duration": format!("{SLEEP_SECS}s")
    }))
    .await;

    assert_eq!(calls, 2, "one interrupted sleep turn, one batch turn");
    assert!(
        elapsed < Duration::from_secs(SLEEP_SECS),
        "the default sleep must be interruptible; elapsed {elapsed:?}"
    );
    assert_eq!(second_prompt, "second");
}
