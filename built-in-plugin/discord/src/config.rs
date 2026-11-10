//! Runtime configuration, assembled from environment variables and CLI flags,
//! plus the per-agent override persisted next to the server data directory.
//!
//! Environment names are deliberately boring so the plugin can be dropped into
//! an existing MCP configuration without a wrapper script.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_API_BASE: &str = "https://discord.com/api/v10";
pub const DEFAULT_GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
pub const DATA_DIR: &str = "data/discord";

/// Per-agent Discord configuration.
///
/// Every field is optional: a missing value inherits the global configuration
/// loaded from the environment. The file lives at
/// `data/discord/config_<safe-agent-id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDiscordConfig {
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub default_channel_id: Option<String>,
}

impl AgentDiscordConfig {
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

/// Discord snowflakes are decimal ids. A channel or guild reference is only
/// ever used as a URL path segment or a JSON string, but rejecting anything
/// else here means a model cannot smuggle a path traversal or a mention syntax
/// into a request.
pub fn normalize_snowflake(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("id cannot be empty");
    }
    if !trimmed.chars().all(|c| c.is_ascii_digit()) {
        bail!("id must be a decimal Discord snowflake, got {raw:?}");
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_token: Option<String>,
    pub default_channel_id: Option<String>,
    /// REST API root, including the version segment.
    pub api_base: String,
    /// Gateway WebSocket URL, including the version and encoding query.
    pub gateway_url: String,
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
    pub default_channel_id: Option<String>,
    pub api_base: Option<String>,
    pub gateway_url: Option<String>,
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
            "--channel-id" | "--default-channel-id" => out.default_channel_id = value(),
            "--api-base" | "--api-url" | "--url" => out.api_base = value(),
            "--gateway-url" | "--gateway" => out.gateway_url = value(),
            _ => {}
        }
    }
    out
}

impl Config {
    pub fn from_env_and_cli(cli: CliOverrides) -> Result<Self> {
        let bot_token = cli
            .bot_token
            .or_else(|| env_opt("DISCORD_BOT_TOKEN"))
            .or_else(|| env_opt("DISCORD_TOKEN"));

        let default_channel_id = cli
            .default_channel_id
            .or_else(|| env_opt("DISCORD_CHANNEL_ID"))
            .or_else(|| env_opt("DISCORD_DEFAULT_CHANNEL_ID"));

        let raw_api_base = cli
            .api_base
            .or_else(|| env_opt("DISCORD_API_BASE"))
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        let api_base = raw_api_base.trim().trim_end_matches('/').to_string();
        if !api_base.starts_with("http://") && !api_base.starts_with("https://") {
            bail!("Discord API base must start with http:// or https://, got {raw_api_base:?}");
        }

        let raw_gateway = cli
            .gateway_url
            .or_else(|| env_opt("DISCORD_GATEWAY_URL"))
            .unwrap_or_else(|| DEFAULT_GATEWAY_URL.to_string());
        let gateway_url = raw_gateway.trim().to_string();
        if !gateway_url.starts_with("ws://") && !gateway_url.starts_with("wss://") {
            bail!("Discord gateway URL must start with ws:// or wss://, got {raw_gateway:?}");
        }

        let timeout = env_opt("DISCORD_REQUEST_TIMEOUT_MS")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(60));

        Ok(Self {
            bot_token,
            default_channel_id,
            api_base,
            gateway_url,
            timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_separate_cli_values() {
        let cli = parse_cli(
            [
                "--token=abc",
                "--channel-id",
                "998877665544332211",
                "--api-base",
                "http://localhost:8081/",
                "--gateway-url=ws://localhost:8082/gateway",
            ]
            .map(String::from)
            .to_vec(),
        );
        assert_eq!(cli.bot_token.as_deref(), Some("abc"));
        assert_eq!(
            cli.default_channel_id.as_deref(),
            Some("998877665544332211")
        );
        assert_eq!(cli.api_base.as_deref(), Some("http://localhost:8081/"));
        assert_eq!(
            cli.gateway_url.as_deref(),
            Some("ws://localhost:8082/gateway")
        );
    }

    #[test]
    fn sanitizes_agent_ids_for_storage() {
        assert_eq!(safe_id("a/b:c.d"), "a_b_c_d");
        assert_eq!(safe_id("plain-id_1"), "plain-id_1");
    }

    #[test]
    fn config_file_path_stays_inside_data_dir() {
        let path = AgentDiscordConfig::file_path("../escape");
        assert!(path.starts_with(DATA_DIR));
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "config____escape.json"
        );
    }

    #[test]
    fn accepts_decimal_snowflakes_and_rejects_the_rest() {
        assert_eq!(
            normalize_snowflake(" 998877665544332211 ").unwrap(),
            "998877665544332211"
        );
        assert!(normalize_snowflake("").is_err());
        assert!(normalize_snowflake("../users/@me").is_err());
        assert!(normalize_snowflake("#general").is_err());
        assert!(normalize_snowflake("998877665544332211/x").is_err());
    }
}
