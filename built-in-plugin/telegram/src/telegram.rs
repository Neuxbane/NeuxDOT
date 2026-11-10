//! A very small Telegram Bot API client.
//!
//! The Bot API is JSON-in/JSON-out: every method is a POST to
//! `{api_base}/bot{token}/{method}` and answers `{"ok":true,"result":...}` or
//! `{"ok":false,"error_code":...,"description":"..."}`. There is no need for a
//! generated client; the tool layer builds the parameter objects.

use anyhow::{Context, Result};
use serde_json::Value;
use thiserror::Error;

use crate::config::{AgentTelegramConfig, Config};

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("invalid bot token: {0}")]
    InvalidToken(String),
    #[error("Telegram API error (HTTP {code}): {description}")]
    Api { code: u16, description: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

/// Replace every `bot<id>:<secret>` occurrence in a string with `bot<redacted>`.
///
/// `reqwest::Error` includes the request URL, and the Telegram Bot API puts the
/// token in the URL path. Without this, a single network failure writes a live
/// bot token into the host log.
pub fn redact_bot_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find("/bot") {
        let after = &rest[idx + 4..];
        // Token ends at the first character outside the Bot API alphabet.
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '_' || c == '-'))
            .unwrap_or(after.len());
        out.push_str(&rest[..idx]);
        out.push_str("/bot<redacted>");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

impl TelegramError {
    /// A message suitable for showing to a user or the model.
    ///
    /// Never contains a token: `Telegram::Network` wraps a `reqwest::Error`
    /// whose `Display` includes the URL, so every branch is passed through
    /// [`redact_bot_tokens`].
    pub fn user_message(&self) -> String {
        match self {
            TelegramError::InvalidToken(_) => {
                "Invalid bot token: expected the value shown by @BotFather.".to_string()
            }
            TelegramError::Api { code: 401, .. } => {
                "Telegram rejected the bot token (HTTP 401 Unauthorized). Check the token or restart the bot with @BotFather.".to_string()
            }
            TelegramError::Api { code: 409, description } => format!(
                "Telegram refused getUpdates (HTTP 409): {description}. \
                 A webhook or another poller is probably active; delete the webhook or stop the other consumer."
            ),
            other => redact_bot_tokens(&other.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct TelegramClient {
    http: reqwest::Client,
    api_base: String,
    default_token: Option<String>,
}

impl TelegramClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .context("building the Telegram HTTP client")?;
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
        agent: Option<&AgentTelegramConfig>,
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

    pub async fn call(
        &self,
        token: &str,
        method: &str,
        params: &Value,
    ) -> Result<Value, TelegramError> {
        validate_token(token)?;
        let url = format!("{}/bot{}/{}", self.api_base, token, method);
        let resp = self.http.post(url).json(params).send().await?;
        let status = resp.status();
        let body: Value = resp.json().await?;

        if body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(body.get("result").cloned().unwrap_or(Value::Null));
        }

        let code = body
            .get("error_code")
            .and_then(Value::as_u64)
            .map(|n| n as u16)
            .unwrap_or_else(|| status.as_u16());
        let description = body
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("unknown Telegram API error")
            .to_string();
        Err(TelegramError::Api { code, description })
    }
}

/// Bot tokens are `<numeric-id>:<secret>` with a restricted alphabet. Reject
/// anything else before it becomes part of a URL path.
fn validate_token(token: &str) -> Result<(), TelegramError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(TelegramError::InvalidToken("empty token".to_string()));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_' || c == '-')
    {
        return Err(TelegramError::InvalidToken(
            "token contains characters outside [A-Za-z0-9:_-]".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_realistic_tokens() {
        assert!(validate_token("123456789:AAH-abc_123-DEF").is_ok());
    }

    #[test]
    fn redacts_tokens_from_error_messages() {
        let raw = "network error: error sending request for url \
                   (https://api.telegram.org/bot123456:AAH-abc_123-DEF/getUpdates)";
        let out = redact_bot_tokens(raw);
        assert!(!out.contains("AAH-abc_123-DEF"), "secret leaked: {out}");
        assert!(out.contains("/bot<redacted>/getUpdates"), "{out}");

        // Two occurrences, and a trailing delimiter must survive.
        let two = "/bot1:AAA/getMe and /bot22:BBB?x=1";
        let out = redact_bot_tokens(two);
        assert!(!out.contains("AAA") && !out.contains("BBB"), "{out}");
        assert!(out.contains("/bot<redacted>/getMe"), "{out}");
        assert!(out.contains("/bot<redacted>?x=1"), "{out}");

        // Ordinary text is untouched.
        assert_eq!(redact_bot_tokens("ok"), "ok");
    }

    #[test]
    fn rejects_path_injection() {
        assert!(validate_token("123:abc/../../etc").is_err());
        assert!(validate_token("123:abc?x=1").is_err());
        assert!(validate_token("").is_err());
    }
}
