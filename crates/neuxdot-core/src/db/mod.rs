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
    let _ = sqlx::query("ALTER TABLE agent_pushed_messages ADD COLUMN is_consumed BOOLEAN DEFAULT 0")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN headers_or_env TEXT DEFAULT NULL")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mcp_servers ADD COLUMN alias TEXT DEFAULT NULL")
        .execute(&pool)
        .await;

    Ok(pool)
}
