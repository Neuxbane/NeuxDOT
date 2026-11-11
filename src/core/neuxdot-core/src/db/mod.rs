pub mod repos;
pub mod schema;
pub mod seed;

pub use repos::Database;
pub use schema::*;
pub use seed::{
    control_plugin_id, ensure_builtin_computer_host, ensure_control_plugin, ensure_control_plugins,
};

use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // If it's a file path, ensure parent directory exists
    if let Some(stripped) = database_url.strip_prefix("sqlite:") {
        let path = Path::new(stripped.split('?').next().unwrap_or(stripped));
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    // Run schema creation
    let schema_sql = include_str!("../../migrations/0001_initial_schema.sql");
    for statement in schema_sql.split(';') {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            sqlx::query(trimmed).execute(&pool).await?;
        }
    }

    // Safely ensure migration columns on agents table
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN allow_output_text BOOLEAN DEFAULT 0")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN output_text_error_message TEXT DEFAULT 'Output Text is not allowed, consider use correct tool or sleep.'")
        .execute(&pool)
        .await;
    let _ = sqlx::query(
        "ALTER TABLE agents ADD COLUMN max_running_history_chars INTEGER DEFAULT 300000",
    )
    .execute(&pool)
    .await;
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN response_hooks_json TEXT DEFAULT '[]'")
        .execute(&pool)
        .await;
    // Per-account Agent Instructions presets (created before the agents column
    // that references it, so the FK resolves on an existing database too).
    let _ = sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agent_instruction_presets (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await;
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN instruction_preset_id TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    // Agent sharing (owner stays agents.account_id).
    let _ = sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agent_shares (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            can_edit BOOLEAN NOT NULL DEFAULT 0,
            can_manage_shares BOOLEAN NOT NULL DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            UNIQUE (agent_id, account_id)
        )
        "#,
    )
    .execute(&pool)
    .await;
    let _ =
        sqlx::query("ALTER TABLE agent_pushed_messages ADD COLUMN is_consumed BOOLEAN DEFAULT 0")
            .execute(&pool)
            .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN headers_or_env TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN alias TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    // Unified Plugins columns on mcp_servers
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN version TEXT NOT NULL DEFAULT '1.0.0'")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN description TEXT DEFAULT ''")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN wasm_hash TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN wasm_binary BLOB DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN manifest_json TEXT DEFAULT '{}'")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN permissions_json TEXT DEFAULT '{}'")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN is_active BOOLEAN DEFAULT 1")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN storage_bytes INTEGER DEFAULT 0")
        .execute(&pool)
        .await;
    let _ = sqlx::query(
        "ALTER TABLE mcp_servers ADD COLUMN created_at DATETIME DEFAULT CURRENT_TIMESTAMP",
    )
    .execute(&pool)
    .await;

    // Base system rows remember the template that produced them so a changed
    // system prompt / instruction preset can be detected (see
    // `replace_base_system_prompt`).
    let _ = sqlx::query("ALTER TABLE agent_history ADD COLUMN source_template TEXT")
        .execute(&pool)
        .await;

    // Roles: every account is a 'member' unless promoted to 'root'.
    let _ = sqlx::query("ALTER TABLE accounts ADD COLUMN role TEXT NOT NULL DEFAULT 'member'")
        .execute(&pool)
        .await;
    // Custom profile picture (data URL or external URL).
    let _ = sqlx::query("ALTER TABLE profiles ADD COLUMN avatar_url TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    // Custom agent picture (data URL or external URL).
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN avatar_url TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    // Existing installs: the bootstrap account named 'root' keeps administrative access.
    let _ = sqlx::query("UPDATE accounts SET role = 'root' WHERE username = 'root'")
        .execute(&pool)
        .await;
    // Per-agent MCP configuration override
    let _ = sqlx::query("ALTER TABLE agent_mcp ADD COLUMN config_override TEXT DEFAULT NULL")
        .execute(&pool)
        .await;

    // Model hyperparameters and context window separation
    let _ = sqlx::query("ALTER TABLE provider_models ADD COLUMN params_json TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ =
        sqlx::query("ALTER TABLE provider_models ADD COLUMN context_window INTEGER DEFAULT NULL")
            .execute(&pool)
            .await;
    let _ = sqlx::query(
        "UPDATE provider_models SET context_window = CAST(json_extract(config_json, '$.context_window') AS INTEGER) WHERE context_window IS NULL AND config_json IS NOT NULL AND json_valid(config_json) = 1 AND json_extract(config_json, '$.context_window') IS NOT NULL"
    )
    .execute(&pool)
    .await;
    let _ = sqlx::query(
        "UPDATE provider_models SET params_json = json_remove(config_json, '$.context_window', '$.name') WHERE params_json IS NULL AND config_json IS NOT NULL AND json_valid(config_json) = 1"
    )
    .execute(&pool)
    .await;

    Ok(pool)
}
