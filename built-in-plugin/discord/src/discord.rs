//! A very small Discord REST client.
//!
//! Every call is `{api_base}/{path}` with `Authorization: Bot {token}` and a
//! `User-Agent` — Discord rejects requests without one. Successful calls answer
//! JSON, failures answer `{"message":"...","code":50035}`. There is no need for
//! a generated client; the tool layer builds the parameter objects.

use anyhow::{Context, Result};
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use thiserror::Error;

use crate::config::{AgentDiscordConfig, Config};

/// Discord requires a unique, versioned User-Agent and rate-limits by it.
/// https://discord.com/developers/docs/reference#user-agent
const USER_AGENT_VALUE: &str = concat!(
    "DiscordBot (https://github.com/NeuxDOT, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

#[derive(Debug, Error)]
pub enum DiscordError {
    #[error("invalid bot token: {0}")]
    InvalidToken(String),
    #[error("Discord API error (HTTP {code}): {description}")]
    Api { code: u16, description: String },
    #[error("rate limited by Discord; retry after {retry_after:.1}s")]
    RateLimited { retry_after: f64 },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

impl DiscordError {
    /// A message suitable for showing to a user or the model.
    pub fn user_message(&self) -> String {
        match self {
            DiscordError::InvalidToken(token) => format!(
                "Invalid bot token {token:?}: expected the token shown in the Discord Developer Portal (Bot tab, Reset Token)."
            ),
            DiscordError::Api { code: 401, .. } => {
                "Discord rejected the bot token (HTTP 401 Unauthorized). Reset the token in the Developer Portal and save it again.".to_string()
            }
            DiscordError::Api { code: 403, .. } => {
                "Discord refused the request (HTTP 403 Forbidden). The bot is missing a permission or cannot view that channel.".to_string()
            }
            DiscordError::Api { code: 404, .. } => {
                "Discord could not find that channel or message (HTTP 404). Check the channel id and that the bot is in the server.".to_string()
            }
            other => other.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct DiscordClient {
    http: reqwest::Client,
    api_base: String,
    default_token: Option<String>,
}

impl DiscordClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .context("building the Discord HTTP client")?;
        Ok(Self {
            http,
            api_base: cfg.api_base.clone(),
            default_token: cfg.bot_token.clone(),
        })
    }

    /// Resolve the credential for one call.
    ///
    /// Precedence: explicit argument, then the agent override, then the global
    /// environment token. This mirrors the rest of the plugin surface, where a
    /// call can be stateless or inherit the configured bot.
    pub fn resolve_token(
        &self,
        explicit: Option<&str>,
        agent: Option<&AgentDiscordConfig>,
    ) -> Option<String> {
        explicit
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                agent
                    .and_then(|c| c.bot_token.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
            .or_else(|| self.default_token.clone())
    }

    pub async fn get(&self, token: &str, path: &str) -> Result<Value, DiscordError> {
        self.request(Some(token), Method::GET, path, None, None)
            .await
    }

    pub async fn post(&self, token: &str, path: &str, body: &Value) -> Result<Value, DiscordError> {
        self.request(Some(token), Method::POST, path, Some(body), None)
            .await
    }

    /// One authenticated call with an arbitrary verb.
    ///
    /// `reason` becomes the `X-Audit-Log-Reason` header, which Discord records
    /// on the audit-log entry for moderation actions. It is percent-encoded
    /// because the header must stay ASCII and Discord caps it at 512 characters.
    pub async fn call(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<&Value>,
        reason: Option<&str>,
    ) -> Result<Value, DiscordError> {
        self.request(Some(token), method, path, body, reason).await
    }

    /// A call that must **not** carry the bot token.
    ///
    /// Executing a webhook is the one public Discord endpoint: sending an
    /// `Authorization` header there is a needless credential exposure and, for a
    /// webhook owned by another application, a 401.
    pub async fn call_anonymous(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, DiscordError> {
        self.request(None, method, path, body, None).await
    }

    async fn request(
        &self,
        token: Option<&str>,
        method: Method,
        path: &str,
        body: Option<&Value>,
        reason: Option<&str>,
    ) -> Result<Value, DiscordError> {
        let url = format!("{}/{}", self.api_base, path.trim_start_matches('/'));
        let mut request = self
            .http
            .request(method, url)
            .header(USER_AGENT, USER_AGENT_VALUE);
        if let Some(token) = token {
            validate_token(token)?;
            request = request.header(AUTHORIZATION, format!("Bot {token}"));
        }
        if let Some(reason) = reason.map(str::trim).filter(|r| !r.is_empty()) {
            request = request.header("X-Audit-Log-Reason", percent_encode(reason, 512));
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let resp = request.send().await?;
        let status = resp.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("retry_after").and_then(Value::as_f64))
                .unwrap_or(1.0);
            return Err(DiscordError::RateLimited { retry_after });
        }

        // 204 No Content is a normal answer for a couple of endpoints; there is
        // no body to parse, so report an explicit null rather than a JSON error.
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }

        let body: Value = resp.json().await?;
        if status.is_success() {
            return Ok(body);
        }

        let description = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown Discord API error")
            .to_string();
        Err(DiscordError::Api {
            code: status.as_u16(),
            description,
        })
    }
}

/// Bot tokens are opaque base64url-ish strings. The token becomes part of an
/// `Authorization` header, which reqwest validates, but rejecting whitespace and
/// control characters up front gives a clearer error than a header parse panic.
fn validate_token(token: &str) -> Result<(), DiscordError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(DiscordError::InvalidToken("empty token".to_string()));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(DiscordError::InvalidToken(
            "token contains whitespace or control characters".to_string(),
        ));
    }
    Ok(())
}

/// Percent-encode everything outside the RFC 3986 unreserved set.
///
/// Used for path segments (custom emoji are `name:id`) and for header values
/// that must stay ASCII (the audit-log reason).
pub fn percent_encode(value: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(value.len());
    for (count, ch) in value.chars().enumerate() {
        if count == max_chars {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("1234", 512), "1234");
        assert_eq!(percent_encode("wave:123456", 512), "wave%3A123456");
        assert_eq!(percent_encode("a b\nc", 512), "a%20b%0Ac");
        assert_eq!(percent_encode("\u{1F44D}", 512), "%F0%9F%91%8D");
        // Truncation is by character, never mid-byte.
        assert_eq!(percent_encode("abcdef", 3), "abc");
    }

    #[test]
    fn accepts_realistic_tokens() {
        // Assembled at runtime and deliberately fake ("112233445566778899"
        // base64-encoded). Keeping it out of the source as one literal stops
        // secret scanners from flagging the test as a leaked credential.
        let token = [
            "MTEyMjMzNDQ1NTY2Nzc4ODk5",
            "GaBcDe",
            "abcdefghijklmnopqrstuvwxyz012345",
        ]
        .join(".");
        assert!(validate_token(&token).is_ok());
        assert!(validate_token("  abc  ").is_ok());
    }

    #[test]
    fn rejects_whitespace_and_empty_tokens() {
        assert!(validate_token("").is_err());
        assert!(validate_token("a b").is_err());
        assert!(validate_token("a\nb").is_err());
    }

    #[test]
    fn token_precedence_is_explicit_then_agent_then_global() {
        let cfg = Config {
            bot_token: Some("global".into()),
            default_channel_id: None,
            api_base: "https://discord.com/api/v10".into(),
            gateway_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
            timeout: std::time::Duration::from_secs(60),
        };
        let client = DiscordClient::new(&cfg).unwrap();
        let agent = AgentDiscordConfig {
            bot_token: Some("agent".into()),
            ..Default::default()
        };

        assert_eq!(
            client
                .resolve_token(Some("explicit"), Some(&agent))
                .as_deref(),
            Some("explicit")
        );
        assert_eq!(
            client.resolve_token(None, Some(&agent)).as_deref(),
            Some("agent")
        );
        assert_eq!(
            client.resolve_token(Some("   "), Some(&agent)).as_deref(),
            Some("agent")
        );
        assert_eq!(
            client
                .resolve_token(None, Some(&Default::default()))
                .as_deref(),
            Some("global")
        );
    }
}
