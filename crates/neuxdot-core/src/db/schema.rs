use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Profile {
    pub account_id: String,
    pub display_name: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Provider {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProviderModel {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub name: Option<String>,
    pub config_json: Option<String>,
    #[sqlx(default)]
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfigInput {
    pub model_id: String,
    pub name: Option<String>,
    pub config_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderWithModels {
    #[serde(flatten)]
    pub provider: Provider,
    pub models: Vec<ProviderModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActiveModelTarget {
    pub id: String, // provider_model_id
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model_id: String,
    pub model_name: Option<String>,
    pub config_json: Option<String>,
}

fn default_hook_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn default_hook_source() -> String {
    "user".to_string()
}
fn default_hook_enabled() -> bool {
    true
}
fn default_max_running_history_chars() -> i64 {
    300_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseHook {
    #[serde(default = "default_hook_id")]
    pub id: String,
    pub message: String,
    #[serde(default = "default_hook_source")]
    pub source: String,
    #[serde(default = "default_hook_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
    pub strategy: String,
    pub system_prompt: Option<String>,
    pub is_active: bool,
    #[sqlx(default)]
    pub allow_output_text: bool,
    #[sqlx(default)]
    pub output_text_error_message: Option<String>,
    #[serde(default = "default_max_running_history_chars")]
    #[sqlx(default)]
    pub max_running_history_chars: i64,
    #[sqlx(default)]
    pub response_hooks_json: Option<String>,
}

impl Agent {
    pub fn max_history_chars(&self) -> usize {
        if self.max_running_history_chars > 0 {
            self.max_running_history_chars as usize
        } else {
            300_000
        }
    }

    pub fn get_response_hooks(&self) -> Vec<ResponseHook> {
        let json_str = self.response_hooks_json.as_deref().unwrap_or("[]");
        if let Ok(hooks) = serde_json::from_str::<Vec<ResponseHook>>(json_str) {
            return hooks;
        }
        if let Ok(strings) = serde_json::from_str::<Vec<String>>(json_str) {
            return strings
                .into_iter()
                .map(|msg| ResponseHook {
                    id: uuid::Uuid::new_v4().to_string(),
                    message: msg,
                    source: "user".to_string(),
                    enabled: true,
                })
                .collect();
        }
        Vec::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentProvider {
    pub agent_id: String,
    pub provider_model_id: String,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct McpServer {
    pub id: String,
    pub account_id: String,
    pub name: String,
    #[sqlx(default)]
    pub alias: Option<String>,
    pub transport: String, // "stdio" or "http"
    pub url_or_command: String,
    #[sqlx(default)]
    pub headers_or_env: Option<String>,
}

pub fn slugify_alias(name: &str) -> String {
    let cleaned = name.replace("[WASM]", "").replace("[wasm]", "");
    let mut s = String::with_capacity(cleaned.len());
    let mut prev_was_underscore = false;

    for c in cleaned.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_was_underscore = false;
        } else if !prev_was_underscore && !s.is_empty() {
            s.push('_');
            prev_was_underscore = true;
        }
    }

    let trimmed = s.trim_matches('_');
    if trimmed.is_empty() {
        "mcp".to_string()
    } else {
        trimmed.to_string()
    }
}

impl McpServer {
    pub fn get_alias(&self) -> String {
        if let Some(a) = &self.alias {
            let trimmed = a.trim();
            if !trimmed.is_empty() {
                return slugify_alias(trimmed);
            }
        }
        slugify_alias(&self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentMcp {
    pub agent_id: String,
    pub mcp_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentPushedMessage {
    pub id: String,
    pub agent_id: String,
    pub mcp_id: Option<String>,
    pub method: String,
    pub params_json: Option<String>,
    #[sqlx(default)]
    pub is_consumed: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentToolPermission {
    pub agent_id: String,
    pub mcp_id: String,
    pub tool_name: String,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentSubscription {
    pub agent_id: String,
    pub uri: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentMemory {
    pub agent_id: String,
    pub content: String,
    pub hash: String,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentHistoryItem {
    pub id: String,
    pub agent_id: String,
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

/// One immutable row of an agent's full lifetime transcript.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentTranscriptItem {
    pub id: String,
    pub agent_id: String,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub origin: String,
    pub turn: i64,
    pub metadata_json: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Extension {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub wasm_hash: String,
    pub wasm_binary: Vec<u8>,
    pub manifest_json: String,
    #[sqlx(default)]
    pub is_active: bool,
    #[sqlx(default)]
    pub storage_bytes: i64,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ExtensionSummary {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub wasm_hash: String,
    pub manifest_json: String,
    #[sqlx(default)]
    pub is_active: bool,
    #[sqlx(default)]
    pub storage_bytes: i64,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ExtensionKvItem {
    pub extension_id: String,
    pub key: String,
    pub value: String,
    pub updated_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_alias() {
        assert_eq!(slugify_alias("Telegram"), "telegram");
        assert_eq!(slugify_alias("Discord Bot"), "discord_bot");
        assert_eq!(slugify_alias("[WASM] Agent Chat Console"), "agent_chat_console");
        assert_eq!(slugify_alias("Filesystem-MCP 1.0"), "filesystem_mcp_1_0");
        assert_eq!(slugify_alias(""), "mcp");
    }

    #[test]
    fn test_mcp_server_get_alias() {
        let server = McpServer {
            id: "1".into(),
            account_id: "acc".into(),
            name: "Telegram Bot".into(),
            alias: Some("tg".into()),
            transport: "http".into(),
            url_or_command: "http://localhost:3000".into(),
            headers_or_env: None,
        };
        assert_eq!(server.get_alias(), "tg");

        let server_no_alias = McpServer {
            id: "2".into(),
            account_id: "acc".into(),
            name: "Discord Bot".into(),
            alias: None,
            transport: "http".into(),
            url_or_command: "http://localhost:3000".into(),
            headers_or_env: None,
        };
        assert_eq!(server_no_alias.get_alias(), "discord_bot");
    }
}
