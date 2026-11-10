//! Runtime configuration, assembled from environment variables and CLI flags,
//! plus the per-agent override persisted next to the server data directory.
//!
//! Environment names are deliberately boring so the plugin can be dropped into
//! an existing MCP configuration without a wrapper script.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";
pub const DATA_DIR: &str = "data/telegram";

/// Per-agent Telegram configuration.
///
/// Every field is optional: a missing value inherits the global configuration
/// loaded from the environment. The file lives at
/// `data/telegram/config_<safe-agent-id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentTelegramConfig {
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub default_chat_id: Option<String>,
    #[serde(default)]
    pub parse_mode: Option<String>,
}

impl AgentTelegramConfig {
    pub fn file_path(agent_id: &str) -> PathBuf {
        PathBuf::from(DATA_DIR).join(format!("config_{}.json", safe_id(agent_id)))
    }

    pub fn load(agent_id: &str) -> Option<Self> {
        let path = Self::file_path(agent_id);
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(agent_id: &str, cfg: &Self) -> std::io::Result<()> {
        std::fs::create_dir_all(DATA_DIR)?;
        let raw = serde_json::to_string_pretty(cfg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(Self::file_path(agent_id), raw)
    }

    pub fn delete(agent_id: &str) -> std::io::Result<()> {
        let path = Self::file_path(agent_id);
        if path.exists() {
            std::fs::remove_file(path)
        } else {
            Ok(())
        }
    }
}

/// Keep an agent id usable as a filename component without changing its
/// identity for the rest of the system.
pub fn safe_id(agent_id: &str) -> String {
    agent_id.replace(['/', '\\', ':', '.'], "_")
}

/// The Telegram Bot API accepts plain text, HTML, or MarkdownV2. Normalising
/// here keeps the MCP tool, the config UI and the environment variable in
/// agreement about the accepted spellings.
pub fn normalize_parse_mode(raw: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("plain")
        || trimmed.eq_ignore_ascii_case("none")
    {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("html") {
        return Ok(Some("HTML".to_string()));
    }
    if trimmed.eq_ignore_ascii_case("markdown") || trimmed.eq_ignore_ascii_case("markdownv2") {
        return Ok(Some("MarkdownV2".to_string()));
    }
    bail!("parse_mode must be HTML or MarkdownV2 (or empty for plain text), got {raw:?}")
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_token: Option<String>,
    pub default_chat_id: Option<String>,
    pub parse_mode: Option<String>,
    /// Bot API root. Override this only for a self-hosted Bot API server.
    pub api_base: String,
    pub timeout: Duration,
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Values recognised on the command line. The environment is preferable
/// because arguments are visible to any process through `ps`.
#[derive(Default)]
pub struct CliOverrides {
    pub bot_token: Option<String>,
    pub default_chat_id: Option<String>,
    pub api_base: Option<String>,
}

pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> CliOverrides {
    let mut out = CliOverrides::default();
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next());
        match key.as_str() {
            "--token" | "--bot-token" => out.bot_token = value(),
            "--chat-id" | "--default-chat-id" => out.default_chat_id = value(),
            "--api-base" | "--api-url" | "--url" => out.api_base = value(),
            _ => {}
        }
    }
    out
}

impl Config {
    pub fn from_env_and_cli(cli: CliOverrides) -> Result<Self> {
        let bot_token = cli
            .bot_token
            .or_else(|| env_opt("TELEGRAM_BOT_TOKEN"))
            .or_else(|| env_opt("TELEGRAM_TOKEN"));

        let default_chat_id = cli
            .default_chat_id
            .or_else(|| env_opt("TELEGRAM_CHAT_ID"))
            .or_else(|| env_opt("TELEGRAM_DEFAULT_CHAT_ID"));

        let raw_api_base = cli
            .api_base
            .or_else(|| env_opt("TELEGRAM_API_BASE"))
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        let api_base = raw_api_base.trim().trim_end_matches('/').to_string();
        if !api_base.starts_with("http://") && !api_base.starts_with("https://") {
            bail!("Telegram API base must start with http:// or https://, got {raw_api_base:?}");
        }

        let parse_mode = normalize_parse_mode(env_opt("TELEGRAM_PARSE_MODE").as_deref())?;

        let timeout = env_opt("TELEGRAM_REQUEST_TIMEOUT_MS")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(60));

        Ok(Self {
            bot_token,
            default_chat_id,
            parse_mode,
            api_base,
            timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parse_modes() {
        assert_eq!(normalize_parse_mode(None).unwrap(), None);
        assert_eq!(normalize_parse_mode(Some("")).unwrap(), None);
        assert_eq!(normalize_parse_mode(Some("plain")).unwrap(), None);
        assert_eq!(
            normalize_parse_mode(Some("html")).unwrap(),
            Some("HTML".into())
        );
        assert_eq!(
            normalize_parse_mode(Some("Markdown")).unwrap(),
            Some("MarkdownV2".into())
        );
        assert_eq!(
            normalize_parse_mode(Some(" markdownv2 ")).unwrap(),
            Some("MarkdownV2".into())
        );
        assert!(normalize_parse_mode(Some("rtf")).is_err());
    }

    #[test]
    fn parses_inline_and_separate_cli_values() {
        let cli = parse_cli(
            [
                "--token=abc",
                "--chat-id",
                "-100123",
                "--api-base",
                "http://localhost:8081/",
            ]
            .map(String::from)
            .to_vec(),
        );
        assert_eq!(cli.bot_token.as_deref(), Some("abc"));
        assert_eq!(cli.default_chat_id.as_deref(), Some("-100123"));
        assert_eq!(cli.api_base.as_deref(), Some("http://localhost:8081/"));
    }

    #[test]
    fn sanitizes_agent_ids_for_storage() {
        assert_eq!(safe_id("a/b:c.d"), "a_b_c_d");
        assert_eq!(safe_id("plain-id_1"), "plain-id_1");
    }

    #[test]
    fn config_file_path_stays_inside_data_dir() {
        let path = AgentTelegramConfig::file_path("../escape");
        assert!(path.starts_with(DATA_DIR));
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "config____escape.json"
        );
    }
}
