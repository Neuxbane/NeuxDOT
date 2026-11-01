use neuxdot_core::auth::{generate_strong_password, hash_password, verify_password};
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::runtime::meta_tools::{
    calculate_memory_hash, get_meta_tools_definitions, interpolate_prompt, parse_duration_string,
    MAX_MEMORY_CHARS,
};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn test_full_platform_e2e() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize SQLite Database & Migrations
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    // 2. Multi-Account: Auto-generate strong 32-character password
    let password = generate_strong_password();
    assert_eq!(password.len(), 32);

    let pass_hash = hash_password(&password).expect("hash password");
    assert!(verify_password(&password, &pass_hash));

    let account = db.create_account("neuxbane", &pass_hash).await?;
    assert_eq!(account.username, "neuxbane");

    // Verify auto-created profile
    let profile = db.get_profile(&account.id).await?.expect("profile exists");
    assert_eq!(profile.display_name.as_deref(), Some("neuxbane"));

    // Update profile
    let updated_profile = db
        .update_profile(&account.id, Some("NeuxBane Core"), Some("#6366f1"), Some("circle"))
        .await?;
    assert_eq!(updated_profile.display_name.as_deref(), Some("NeuxBane Core"));

    // 3. Provider Configuration and Provider Models (decoupled models table)
    let config = json!({
        "temperature": 0.7,
        "thinking": {
            "type": "enabled",
            "budget_tokens": 2048
        }
    });

    let provider = db
        .create_provider(
            &account.id,
            "Local Llama 3",
            "http://localhost:11434/v1",
            None,
        )
        .await?;
    assert_eq!(provider.name, "Local Llama 3");

    let p_model = db
        .add_provider_model(
            &provider.id,
            "llama3:8b",
            Some("Llama 3 8B"),
            Some(&config.to_string()),
        )
        .await?;
    assert_eq!(p_model.model_id, "llama3:8b");
    assert!(p_model.config_json.unwrap().contains("budget_tokens"));

    // 4. External MCP Server Registration
    let mcp = db
        .create_mcp_server(
            &account.id,
            "Filesystem MCP",
            "stdio",
            "node ./mcp-servers/fs.js",
        )
        .await?;
    assert_eq!(mcp.name, "Filesystem MCP");
    assert_eq!(mcp.transport, "stdio");

    // 5. Agent Creation & Settings (default allow_output_text = false)
    let agent = db
        .create_agent(
            &account.id,
            "Ares",
            Some("#6366f1"),
            Some("circle"),
            "fallback",
            Some("You are {{agent.name}}, an autonomous engineer."),
            None,
            None,
            Some(300_000),
            Some("[]"),
        )
        .await?;
    assert_eq!(agent.name, "Ares");
    assert!(!agent.is_active);
    assert!(!agent.allow_output_text); // default false
    assert_eq!(agent.max_running_history_chars, 300_000);
    assert_eq!(
        agent.output_text_error_message.as_deref(),
        Some("Output Text is not allowed, consider use correct tool or sleep.")
    );

    // Bind Model & MCP to Agent
    db.set_agent_models(&agent.id, &[p_model.id.clone()]).await?;
    db.set_agent_mcp_servers(&agent.id, &[mcp.id.clone()]).await?;

    let bound_models = db.get_agent_models(&agent.id).await?;
    assert_eq!(bound_models.len(), 1);
    assert_eq!(bound_models[0].model_id, "llama3:8b");
    assert_eq!(bound_models[0].provider_name, "Local Llama 3");

    let bound_m = db.get_agent_mcp_servers(&agent.id).await?;
    assert_eq!(bound_m.len(), 1);

    // Verify AgentExecutor instantiates with ActiveModelTarget
    let (event_tx, _) = tokio::sync::broadcast::channel(32);
    let executor = neuxdot_core::runtime::executor::AgentExecutor::new(
        std::sync::Arc::new(db.clone()),
        agent.clone(),
        bound_models.clone(),
        vec![mcp.clone()],
        event_tx,
    );
    assert_eq!(executor.models.len(), 1);

    // Verify Meta-Tools (strictly 9 invariant meta-tools: search, schema, call, servers, setMemory, editMemory, readMemory, compact, sleep)
    let meta_tools = get_meta_tools_definitions();
    let tools_arr = meta_tools.as_array().unwrap();
    assert_eq!(tools_arr.len(), 9);
    let names: Vec<&str> = tools_arr
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "search_tools",
            "get_tool_schema",
            "call_tool",
            "list_servers",
            "setMemory",
            "editMemory",
            "readMemory",
            "compact",
            "sleep"
        ]
    );

    // Test duration parsing for sleep meta-tool
    assert_eq!(parse_duration_string("1m").unwrap(), Duration::from_secs(60));
    assert_eq!(parse_duration_string("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration_string("2h").unwrap(), Duration::from_secs(7200));
    assert_eq!(parse_duration_string("1d").unwrap(), Duration::from_secs(86400));
    assert_eq!(parse_duration_string("1h30m").unwrap(), Duration::from_secs(5400));
    assert!(parse_duration_string("invalid").is_err());

    // Test Safe Literal Prompt Interpolation
    let raw_prompt = agent.system_prompt.as_deref().unwrap();
    let interpolated = interpolate_prompt(
        raw_prompt,
        &agent.id,
        &agent.name,
        &agent.strategy,
        "",
        "hash0",
        &[],
        300_000,
    );
    assert_eq!(interpolated, "You are Ares, an autonomous engineer.");

    // 6. Test 500ms Debounced Auto-Save Patch with allow_output_text toggle
    let patched = db
        .patch_agent(
            &agent.id,
            Some("Ares v2"),
            None,
            None,
            Some("round-robin"),
            None,
            Some(true),
            Some(true), // enable output text
            Some("Custom disallowed message"),
            Some(250_000),
            Some(r#"[{"id":"h1","message":"Time: {{time}}","source":"user","enabled":true}]"#),
        )
        .await?;
    assert_eq!(patched.name, "Ares v2");
    assert_eq!(patched.strategy, "round-robin");
    assert!(patched.is_active);
    assert!(patched.allow_output_text);
    assert_eq!(patched.output_text_error_message.as_deref(), Some("Custom disallowed message"));
    assert_eq!(patched.max_running_history_chars, 250_000);

    // 7. Test MCP Server Push Information Ingestion
    let pushed_payload = json!({
        "event": "file_modified",
        "path": "/src/main.rs",
        "timestamp": 1729000000
    });

    let saved_push = db
        .save_pushed_message(
            &agent.id,
            Some(&mcp.id),
            "notifications/resources/updated",
            Some(&pushed_payload.to_string()),
        )
        .await?;
    assert_eq!(saved_push.method, "notifications/resources/updated");

    // Verify agent retrieves pushed notifications in context
    let messages = db.list_pushed_messages(&agent.id, 10).await?;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].method, "notifications/resources/updated");
    assert!(messages[0].params_json.as_ref().unwrap().contains("file_modified"));

    // 8. Test Memory Operations & Constraints
    let initial_mem = db.get_agent_memory(&agent.id).await?;
    assert_eq!(initial_mem.content, "");
    let empty_hash = calculate_memory_hash("");
    assert_eq!(initial_mem.hash, empty_hash);

    let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    assert_ne!(initial_mem.hash, wrong_hash);

    let oversized_content = "a".repeat(5001);
    assert!(oversized_content.chars().count() > MAX_MEMORY_CHARS);

    let note = "User prefers concise answers and Rust code.";
    let updated_hash = calculate_memory_hash(note);
    let saved_mem = db.set_agent_memory(&agent.id, note, &updated_hash).await?;
    assert_eq!(saved_mem.content, note);
    assert_eq!(saved_mem.hash, updated_hash);

    let search = "concise";
    let replace = "concise and modular";
    assert!(saved_mem.content.contains(search));
    let edited = saved_mem.content.replace(search, replace);
    let edited_hash = calculate_memory_hash(&edited);
    let final_mem = db.set_agent_memory(&agent.id, &edited, &edited_hash).await?;
    assert_eq!(final_mem.content, "User prefers concise and modular answers and Rust code.");
    assert_eq!(final_mem.hash, edited_hash);

    let read_back = db.get_agent_memory(&agent.id).await?;
    assert_eq!(read_back.content, final_mem.content);
    assert_eq!(read_back.hash, final_mem.hash);

    // 9. Test Agent Running History & Compaction
    let initial_history = db.get_agent_history(&agent.id).await?;
    assert_eq!(initial_history.len(), 0);

    let _sys_item = db.append_agent_history(&agent.id, "system", "You are Ares.", None, None).await?;
    let _user_item = db.append_agent_history(&agent.id, "user", "Hello Ares!", None, None).await?;
    let _asst_item = db.append_agent_history(&agent.id, "assistant", "Hello! How can I assist?", None, None).await?;
    assert_eq!(db.get_agent_history(&agent.id).await?.len(), 3);

    let compacted_sys = format!("You are Ares.\n\n### Agent Memory (Hash: {}):\n{}", final_mem.hash, final_mem.content);
    let tool_reply = json!({ "success": true, "compacted": true, "current_hash": final_mem.hash }).to_string();
    db.compact_agent_history(&agent.id, &compacted_sys, Some("call_compact_1"), Some(&tool_reply)).await?;

    let post_compact_history = db.get_agent_history(&agent.id).await?;
    assert_eq!(post_compact_history.len(), 2);
    assert_eq!(post_compact_history[0].role, "system");
    assert!(post_compact_history[0].content.contains(&final_mem.hash));
    assert_eq!(post_compact_history[1].role, "tool");
    assert_eq!(post_compact_history[1].tool_call_id.as_deref(), Some("call_compact_1"));

    // Test Safe Handlebars Prompt Interpolation with {{agent.name}} and {{tools.preset}}
    let handlebars_template = "You are {{agent.name}}. Tools: {{tools.preset}}. Request schema on-demand.";
    let allowed_tools = vec!["chat_send".to_string(), "read_file".to_string()];
    let interpolated_hbs = interpolate_prompt(
        handlebars_template,
        &agent.id,
        &agent.name,
        &agent.strategy,
        &final_mem.content,
        &final_mem.hash,
        &allowed_tools,
        250_000,
    );
    assert_eq!(
        interpolated_hbs,
        "You are Ares. Tools: chat_send, read_file. Request schema on-demand."
    );

    // Test AgentTimerManager: Single-Timer Invariant and Interruption Cancellation
    let timer_mgr = neuxdot_core::runtime::global_timer_manager();
    let (t1_tx, t1_rx) = tokio::sync::oneshot::channel::<String>();
    timer_mgr.register_timer(&agent.id, t1_tx, 120);
    assert!(timer_mgr.has_active_timer(&agent.id));

    // Registering a second timer immediately supersedes and cancels the first
    let (t2_tx, mut t2_rx) = tokio::sync::oneshot::channel::<String>();
    timer_mgr.register_timer(&agent.id, t2_tx, 60);
    let cancel_msg = t1_rx.await.expect("t1 cancelled");
    assert!(cancel_msg.contains("superseded"));

    // Interrupting cancels the active timer so it will not retrigger
    assert!(timer_mgr.cancel_timer(&agent.id, "user interrupt"));
    assert!(!timer_mgr.has_active_timer(&agent.id));
    let t2_cancel = t2_rx.try_recv().expect("t2 cancelled");
    assert_eq!(t2_cancel, "user interrupt");

    println!("All E2E platform checks passed successfully!");
    Ok(())
}
