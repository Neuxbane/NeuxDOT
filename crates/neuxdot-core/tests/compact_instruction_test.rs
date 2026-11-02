use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;

#[tokio::test]
async fn compact_installs_new_instruction_and_keeps_running_history_intact_before_it() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Database::new(pool);
    let (acc, _) = db.get_or_init_root_account().await.unwrap();
    // create_agent(account_id, name, avatar_color, avatar_shape, avatar_url, strategy,
    //              system_prompt, allow_output_text, output_text_error_message,
    //              max_running_history_chars, response_hooks_json)
    let agent = db
        .create_agent(&acc.id, "A", None, None, None, "fallback", Some("OLD INSTRUCTION"), None, None, None, None, None)
        .await
        .unwrap();

    // Materialise row 0 the way run_turn does.
    db.append_agent_history(&agent.id, "system", "OLD INSTRUCTION", None, None).await.unwrap();
    db.append_agent_history(&agent.id, "user", "hello", None, None).await.unwrap();

    // User selects a new instruction preset -> only agents.system_prompt changes.
    // patch_agent(id, name, avatar_color, avatar_shape, avatar_url, strategy,
    //             system_prompt, is_active, allow_output_text,
    //             output_text_error_message, max_running_history_chars, response_hooks_json)
    db.patch_agent(
        &agent.id, None, None, None, None, None, Some("NEW INSTRUCTION"), None, None, None, None, None, None,
    )
    .await
    .unwrap();

    // Running history must be untouched (cache-safe).
    let before = db.get_agent_history(&agent.id).await.unwrap();
    assert_eq!(before[0].content, "OLD INSTRUCTION", "history must NOT change mid-run");
    assert_eq!(before.len(), 2);

    // Compact installs the new instruction.
    db.compact_agent_history(&agent.id, "NEW INSTRUCTION", "summary", None, &[]).await.unwrap();
    let after = db.get_agent_history(&agent.id).await.unwrap();
    assert_eq!(after[0].role, "system");
    assert_eq!(after[0].content, "NEW INSTRUCTION");
    assert_eq!(after.iter().filter(|h| h.content == "NEW INSTRUCTION").count(), 1);
    println!("OK: old kept during run, new installed on compact");
}
