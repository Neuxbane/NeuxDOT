//! The system instruction + persistent memory block must be rendered ONCE and
//! then carried as row 0 of the working history — not re-injected every turn.
//!
//! Re-rendering it per turn would (a) duplicate tokens and (b) invalidate the
//! provider's prefix cache on every request. This test captures the real HTTP
//! bodies sent to a mock provider across two turns and asserts the system
//! message appears exactly once per request and is byte-identical between them.

use std::sync::{Arc, Mutex};

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ActiveModelTarget;
use neuxdot_core::runtime::executor::AgentExecutor;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Mock provider that records every request body and always answers with text.
async fn mock_provider() -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bodies);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 1 << 20];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            if let Some(body) = raw.split("\r\n\r\n").nth(1) {
                if let Ok(v) = serde_json::from_str::<Value>(body) {
                    sink.lock().unwrap().push(v);
                }
            }
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
        api_key: Some("k".into()),
        model_id: "mock".into(),
        model_name: None,
        config_json: None,
        params_json: None,
        context_window: Some(128_000),
    }
}

fn system_messages(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m["role"] == "system")
                .filter_map(|m| m["content"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn system_prompt_and_memory_are_rendered_once_not_per_turn() {
    let (base_url, bodies) = mock_provider().await;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Arc::new(Database::new(pool));
    let (account, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &account.id,
            "Stable",
            None,
            None,
            None,
            "fallback",
            Some("You are {{agent.name}}. Memory: {{agent.memory}}"),
            Some(true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Seed the persistent instructions block so it is part of the render.
    let hash = neuxdot_core::runtime::meta_tools::calculate_memory_hash("STANDING ORDERS");
    db.set_agent_memory(&agent.id, "STANDING ORDERS", &hash)
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

    executor.run_turn("first").await.unwrap();
    executor.run_turn("second").await.unwrap();

    let captured = bodies.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "expected one completion per turn");

    for (i, body) in captured.iter().enumerate() {
        let systems = system_messages(body);
        assert_eq!(
            systems.len(),
            1,
            "turn {i} must carry exactly one system message, got {}",
            systems.len()
        );
        assert!(
            systems[0].contains("STANDING ORDERS"),
            "turn {i} system message must contain the memory block: {}",
            systems[0]
        );
    }

    assert_eq!(
        system_messages(&captured[0]),
        system_messages(&captured[1]),
        "the system prefix must be byte-identical across turns for cache stability"
    );

    // And the working history holds exactly one system row.
    let history = db.get_agent_history(&agent.id).await.unwrap();
    let system_rows = history.iter().filter(|h| h.role == "system").count();
    assert_eq!(
        system_rows, 1,
        "the system instruction must be stored once, not appended per turn"
    );
}
