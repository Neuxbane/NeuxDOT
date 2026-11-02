pub mod repos;
pub mod schema;

pub use repos::Database;
pub use schema::*;

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
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN max_running_history_chars INTEGER DEFAULT 300000")
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
    let _ = sqlx::query("ALTER TABLE agent_pushed_messages ADD COLUMN is_consumed BOOLEAN DEFAULT 0")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN headers_or_env TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN alias TEXT DEFAULT NULL")
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

    Ok(pool)
}
