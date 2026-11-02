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
        .update_profile(
            &account.id,
            Some("NeuxBane Core"),
            Some("#6366f1"),
            Some("circle"),
            Some("data:image/png;base64,AAAA"),
        )
        .await?;
    assert_eq!(updated_profile.display_name.as_deref(), Some("NeuxBane Core"));
    assert_eq!(
        updated_profile.avatar_url.as_deref(),
        Some("data:image/png;base64,AAAA")
    );

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
            Some("fs"),
            "stdio",
            "node ./mcp-servers/fs.js",
            None,
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
            None,
            "fallback",
            Some("You are {{agent.name}}, an autonomous engineer."),
            None,
            None,
            Some(300_000),
            Some("[]"),
            None,
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

    // Verify Meta-Tools (11: search, schema, call, servers, setMemory, editMemory,
    // readMemory, compact, sleep, history_search, history_inspect)
    let meta_tools = get_meta_tools_definitions();
    let tools_arr = meta_tools.as_array().unwrap();
    assert_eq!(tools_arr.len(), 11);
    assert!(tools_arr
        .iter()
        .any(|t| t["function"]["name"] == "history_search"));
    assert!(tools_arr
        .iter()
        .any(|t| t["function"]["name"] == "history_inspect"));
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
            "sleep",
            "history_search",
            "history_inspect"
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
            None,
            Some("round-robin"),
            None,
            Some(true),
            Some(true), // enable output text
            Some("Custom disallowed message"),
            Some(250_000),
            Some(r#"[{"id":"h1","message":"Time: {{time}}","source":"user","enabled":true}]"#),
            None,
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

    let _sys_item = db.append_agent_history(&agent.id, "system", "You are Ares.", None, None, None).await?;
    let _user_item = db.append_agent_history(&agent.id, "user", "Hello Ares!", None, None, None).await?;
    let _asst_item = db.append_agent_history(&agent.id, "assistant", "Hello! How can I assist?", None, None, None).await?;
    // Mirror what the executor does: every turn is also archived to the
    // append-only transcript.
    db.append_transcript(&agent.id, "system", "You are Ares.", None, None, "system", 1, None).await?;
    db.append_transcript(&agent.id, "user", "Hello Ares!", None, None, "turn", 1, None).await?;
    db.append_transcript(&agent.id, "assistant", "Hello! How can I assist?", None, None, "turn", 1, None).await?;
    assert_eq!(db.get_agent_history(&agent.id).await?.len(), 3);

    let summary = format!("Memory (Hash: {}):\n{}", final_mem.hash, final_mem.content);
    let compact_assistant = json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "id": "call_compact_1",
            "type": "function",
            "function": { "name": "compact", "arguments": "{}" }
        }]
    });
    let replies = vec![(
        "call_compact_1".to_string(),
        json!({ "success": true }).to_string(),
    )];
    // Compaction reinstalls a freshly rendered system instruction: this is how a
    // changed instruction preset / memory becomes live for a running agent.
    let new_instruction = "You are Ares.\n### Agent Memory (Hash: newhash):\nupdated memory";
    db.compact_agent_history(
        &agent.id,
        new_instruction,
        &summary,
        Some(&compact_assistant),
        &replies,
    )
    .await?;

    let post_compact_history = db.get_agent_history(&agent.id).await?;
    // Exact persisted order: [new system instruction] [assistant] [tool] [summary].
    // The compact tool reply MUST be adjacent to its assistant message,
    // otherwise history repair mints a bogus "interrupted" response.
    assert_eq!(post_compact_history.len(), 4);
    assert_eq!(post_compact_history[0].role, "system");
    assert_eq!(
        post_compact_history[0].content, new_instruction,
        "compact must install the NEW instruction, not the old row 0"
    );
    assert_eq!(post_compact_history[1].role, "assistant");
    assert!(post_compact_history[1]
        .tool_calls_json
        .as_deref()
        .unwrap()
        .contains("call_compact_1"));
    assert_eq!(post_compact_history[2].role, "tool");
    assert_eq!(post_compact_history[2].tool_call_id.as_deref(), Some("call_compact_1"));
    assert_eq!(post_compact_history[3].role, "system");
    assert!(post_compact_history[3].content.contains("Compaction Summary"));
    assert!(post_compact_history[3].content.contains(&final_mem.hash));
    // The new instruction must appear exactly once (never duplicated).
    assert_eq!(
        post_compact_history
            .iter()
            .filter(|h| h.content == new_instruction)
            .count(),
        1
    );

    // The append-only transcript must retain what compaction removed.
    let transcript = db.get_agent_transcript(&agent.id).await?;
    assert!(
        transcript.iter().any(|t| t.role == "user" && t.content == "Hello Ares!"),
        "compaction must not delete transcript history"
    );
    assert!(
        transcript.iter().any(|t| t.content == "Hello! How can I assist?"),
        "transcript must retain pre-compaction assistant turns"
    );
    for (i, item) in transcript.iter().enumerate() {
        assert_eq!(item.seq, (i + 1) as i64);
    }
    let exported = db.export_agent_transcript(&agent.id).await?;
    assert_eq!(exported.len(), transcript.len());
    assert_eq!(exported[0]["role"], "system");

    // Pruning must delete whole assistant-tool-call groups, never leaving a
    // tool row without its assistant parent (or vice versa).
    let tool_calls = json!([
        { "id": "call_p1", "type": "function", "function": { "name": "sleep", "arguments": "{}" } },
        { "id": "call_p2", "type": "function", "function": { "name": "computer.list", "arguments": "{}" } }
    ])
    .to_string();
    db.append_agent_history(&agent.id, "user", "next turn", None, None, None).await?;
    db.append_agent_history(&agent.id, "assistant", "", Some(&tool_calls), None, None).await?;
    db.append_agent_history(&agent.id, "tool", "slept", None, Some("call_p1"), None).await?;
    db.append_agent_history(&agent.id, "tool", "listed", None, Some("call_p2"), None).await?;
    db.append_agent_history(&agent.id, "user", "after tools", None, None, None).await?;

    // Budget only large enough for the system prompt + the trailing user turns.
    let pruned = db
        .prune_agent_history_to_limit(&agent.id, post_compact_history[0].content.chars().count() + 30)
        .await?;
    assert!(pruned > 0);
    let remaining = db.get_agent_history(&agent.id).await?;
    assert_eq!(remaining[0].role, "system");
    assert!(
        remaining.iter().all(|h| h.tool_call_id.is_none()),
        "pruning left orphaned tool rows: {:?}",
        remaining.iter().map(|h| (&h.role, &h.tool_call_id)).collect::<Vec<_>>()
    );

    // --- History search / inspect round-trip over the append-only transcript ---
    let big_result = format!("HEAD-{}--MIDDLE--{}-TAIL", "x".repeat(5000), "y".repeat(5000));
    let row = db
        .append_transcript(
            &agent.id,
            "tool",
            &big_result,
            None,
            Some("call_big"),
            "tool",
            2,
            None,
        )
        .await?;

    // The transcript keeps FULL fidelity (distillation source).
    assert_eq!(row.content.chars().count(), big_result.chars().count());
    assert_eq!(row.content, big_result);

    // history_search finds it and reports an offset.
    let hits = db.search_transcript(&agent.id, "MIDDLE", 100).await?;
    assert!(!hits.is_empty());
    let hit = hits.iter().find(|h| h["id"] == row.id.as_str()).expect("hit for row");
    let offset = hit["offset"].as_u64().unwrap() as usize;
    assert_eq!(hit["role"], "tool");
    assert!(hit["time"].as_i64().unwrap() > 0);
    assert!(hit["preview"].as_str().unwrap().starts_with("MIDDLE"));

    // history_inspect reconstructs exactly the omitted region.
    let (total, slice) = db
        .inspect_transcript(&agent.id, &row.id, offset, offset + 6)
        .await?
        .expect("inspect");
    assert_eq!(total, big_result.chars().count());
    assert_eq!(slice, "MIDDLE");

    // Out-of-range requests clamp instead of panicking.
    let (total2, empty) = db
        .inspect_transcript(&agent.id, &row.id, total + 100, total + 200)
        .await?
        .expect("inspect clamped");
    assert_eq!(total2, total);
    assert!(empty.is_empty());

    // Unknown ids are reported, not fabricated.
    assert!(db
        .inspect_transcript(&agent.id, "does-not-exist", 0, 10)
        .await?
        .is_none());

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
