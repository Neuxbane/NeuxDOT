//! Regression tests for switching an agent's Agent Instructions preset.
//!
//! Bug being locked down: selecting a preset in the UI persisted the text into
//! `agents.system_prompt`, but the conversation's base `system` row (written
//! once, at the start) was never refreshed. The dropdown showed the preset
//! while the model kept obeying the previous instructions.

use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;

/// The base instruction row is the first `system` row of the working context.
async fn base_instruction(db: &Database, agent_id: &str) -> String {
    db.get_agent_history(agent_id)
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.role == "system")
        .map(|h| h.content)
        .expect("a base system row must exist")
}

#[tokio::test]
async fn base_system_prompt_refreshes_when_the_template_changes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Database::new(pool);
    let (acc, _) = db.get_or_init_root_account().await.unwrap();

    // Agent starts on the default instruction.
    let agent = db
        .create_agent(
            &acc.id, "A", None, None, None, "fallback",
            Some("DEFAULT INSTRUCTION"), None, None, None, None, None,
        )
        .await
        .unwrap();

    // Row 0 is materialised from the default template, recording that template.
    db.append_agent_history(
        &agent.id, "system", "DEFAULT INSTRUCTION", None, None, Some("DEFAULT INSTRUCTION"),
    )
    .await
    .unwrap();
    db.append_agent_history(&agent.id, "user", "hello", None, None, None)
        .await
        .unwrap();

    assert_eq!(base_instruction(&db, &agent.id).await, "DEFAULT INSTRUCTION");

    // User picks a preset: `agents.system_prompt` becomes the preset content.
    db.patch_agent(
        &agent.id, None, None, None, None, None,
        Some("PRESET INSTRUCTION"), None, None, None, None, None, None,
    )
    .await
    .unwrap();

    // The runtime detects that the stored template no longer matches and
    // replaces the base row in place.
    let stored = db
        .get_agent_history(&agent.id)
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.role == "system")
        .unwrap()
        .source_template;
    assert_eq!(stored.as_deref(), Some("DEFAULT INSTRUCTION"));

    let updated = db
        .replace_base_system_prompt(&agent.id, "PRESET INSTRUCTION", "PRESET INSTRUCTION")
        .await
        .unwrap();
    assert!(updated, "the base row must be replaced");

    assert_eq!(
        base_instruction(&db, &agent.id).await,
        "PRESET INSTRUCTION",
        "the preset must win over the previous default"
    );

    // The rest of the conversation is untouched.
    let hist = db.get_agent_history(&agent.id).await.unwrap();
    assert_eq!(hist.len(), 2, "only the base row changes");
    assert_eq!(hist[1].content, "hello");

    // And it is no longer reported as stale.
    let template = hist
        .into_iter()
        .find(|h| h.role == "system")
        .unwrap()
        .source_template;
    assert_eq!(template.as_deref(), Some("PRESET INSTRUCTION"));
}

#[tokio::test]
async fn rows_without_a_template_are_left_alone() {
    // Legacy rows (written before `source_template` existed) must not be
    // guessed at or clobbered — a compaction summary could be row 0.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let db = Database::new(pool);
    let (acc, _) = db.get_or_init_root_account().await.unwrap();
    let agent = db
        .create_agent(
            &acc.id, "A", None, None, None, "fallback",
            Some("SOMETHING"), None, None, None, None, None,
        )
        .await
        .unwrap();

    db.append_agent_history(&agent.id, "system", "### [Compaction Summary]\nold", None, None, None)
        .await
        .unwrap();

    let hist = db.get_agent_history(&agent.id).await.unwrap();
    assert_eq!(hist[0].source_template, None);
    assert_eq!(hist[0].content, "### [Compaction Summary]\nold");
}
