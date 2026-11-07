use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::{ActiveModelTarget, ModelConfigInput};
use neuxdot_core::runtime::router::ProviderRouter;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn test_db_migration_backfills_params_and_context_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Database::new(pool);
    let (acc, _) = db.get_or_init_root_account().await.unwrap();

    let provider = db
        .create_provider(
            &acc.id,
            "Test Provider",
            "https://api.openai.com/v1",
            Some("sk-test"),
        )
        .await
        .unwrap();

    // Insert using config_json containing both completion hyperparameters and UI metadata
    let model = db
        .add_provider_model(
            &provider.id,
            "test-model-1",
            Some("Test Model 1"),
            Some(r#"{"context_window": 128000, "name": "Test Model 1", "temperature": 0.7, "max_tokens": 4096}"#),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(model.context_window, Some(128000));
    assert!(model.params_json.is_some());
    let params: Value = serde_json::from_str(model.params_json.as_deref().unwrap()).unwrap();
    assert_eq!(params["temperature"], 0.7);
    assert_eq!(params["max_tokens"], 4096);
    assert!(params.get("context_window").is_none());
    assert!(params.get("name").is_none());

    // Create an agent and bind this model, then get agent models
    let agent = db
        .create_agent(
            &acc.id,
            "Agent 1",
            None,
            None,
            None,
            "fallback",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    db.set_agent_models(&agent.id, &[model.id.clone()]).await.unwrap();
    let targets = db.get_agent_models(&agent.id).await.unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].context_window, Some(128000));
    assert!(targets[0].params_json.is_some());
}

#[tokio::test]
async fn test_router_does_not_forward_unknown_metadata_key_to_provider() {
    // Spin up a local mock HTTP server that captures incoming JSON payload
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let req_str = String::from_utf8_lossy(&buf[..n]);

        // Parse JSON body from HTTP request
        let body_str = req_str.split("\r\n\r\n").nth(1).unwrap_or("");
        let body_json: Value = serde_json::from_str(body_str).unwrap();

        // Send a 200 OK mock response
        let resp_body = json!({
            "id": "chatcmpl-test",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "ok"
                }
            }]
        })
        .to_string();

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        socket.write_all(response.as_bytes()).await.unwrap();

        body_json
    });

    let target = ActiveModelTarget {
        id: "pm_test".to_string(),
        provider_id: "p_test".to_string(),
        provider_name: "Mock Provider".to_string(),
        base_url: format!("http://{}", local_addr),
        api_key: Some("key-test".to_string()),
        model_id: "claude-3-7-sonnet".to_string(),
        model_name: Some("Claude 3.7 Sonnet".to_string()),
        config_json: Some(
            json!({
                "supports_thinking": true,
                "icon": "brain-spark",
                "description": "Smart model",
                "context_window": 200000
            })
            .to_string(),
        ),
        params_json: Some(
            json!({
                "temperature": 0.2,
                "max_tokens": 16000
            })
            .to_string(),
        ),
        context_window: Some(200000),
    };

    let router = ProviderRouter::new(vec![target], "fallback");
    let messages = vec![json!({"role": "user", "content": "Hello world"})];

    let result = router
        .execute_chat_completion(messages, None, |_| {})
        .await;

    assert!(result.is_ok(), "Chat completion must succeed");

    let received_payload = server_task.await.unwrap();

    // Verify completion parameters ARE forwarded
    assert_eq!(received_payload["model"], "claude-3-7-sonnet");
    assert_eq!(received_payload["temperature"], 0.2);
    assert_eq!(received_payload["max_tokens"], 16000);

    // Verify unknown UI metadata keys are NOT forwarded
    assert!(
        received_payload.get("supports_thinking").is_none(),
        "supports_thinking must not be sent to provider payload"
    );
    assert!(
        received_payload.get("icon").is_none(),
        "icon must not be sent to provider payload"
    );
    assert!(
        received_payload.get("description").is_none(),
        "description must not be sent to provider payload"
    );
    assert!(
        received_payload.get("context_window").is_none(),
        "context_window must not be sent to provider payload"
    );
}

#[tokio::test]
async fn test_set_provider_models_with_explicit_params_json() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Database::new(pool);
    let (acc, _) = db.get_or_init_root_account().await.unwrap();

    let provider = db
        .create_provider(
            &acc.id,
            "Provider B",
            "https://api.openai.com/v1",
            None,
        )
        .await
        .unwrap();

    let models_input = vec![
        ModelConfigInput {
            model_id: "model-explicit".to_string(),
            name: Some("Explicit Model".to_string()),
            config_json: Some(r#"{"supports_thinking": true}"#.to_string()),
            params_json: Some(r#"{"temperature": 0.8}"#.to_string()),
            context_window: Some(65536),
        },
    ];

    db.set_provider_models(&provider.id, &models_input).await.unwrap();

    let models = db.list_provider_models(&provider.id).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].context_window, Some(65536));
    assert_eq!(models[0].params_json.as_deref(), Some(r#"{"temperature": 0.8}"#));
    assert_eq!(models[0].config_json.as_deref(), Some(r#"{"supports_thinking": true}"#));
}
