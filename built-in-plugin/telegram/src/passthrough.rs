//! `telegram_api`: one guarded passthrough for methods the table does not name.
//!
//! The table covers the documented Bot API, but Telegram adds methods between
//! plugin builds, and a bot with a local Bot API server has a few extra. This
//! tool is a shape guard, not a policy gate: the bot's own Telegram rights
//! remain the authority, exactly as they are for the typed tools.
//!
//! What is rejected: a method name that is not a plain PascalCase word (it
//! becomes part of the request URL, so anything else is path injection), and
//! the handful of methods that would hand a plugin raw file bytes or move the
//! bot between servers.

use serde_json::{Map, Value};

/// Methods that must never be reachable from the passthrough.
///
/// `getUpdates` alone: the live poller owns the offset, and a second consumer
/// makes Telegram answer 409 while silently dropping messages from whichever
/// consumer loses the race. Every other method is either typed in the table
/// (so this is redundant for it) or genuinely uncovered, which is exactly what
/// this tool is for.
const FORBIDDEN_METHODS: &[&str] = &["getUpdates"];

pub struct Request {
    pub method: String,
    pub params: Value,
}

/// Validate and assemble one passthrough request.
pub fn build(args: &Map<String, Value>) -> Result<Request, String> {
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "'method' is required, e.g. sendMessage".to_string())?;

    validate_method(method)?;

    let params = match args.get("params") {
        Some(Value::Null) | None => Value::Object(Map::new()),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => return Err("'params' must be a JSON object".to_string()),
    };

    Ok(Request {
        method: method.to_string(),
        params,
    })
}

/// A Bot API method name: a camelCase word, nothing else.
pub fn validate_method(method: &str) -> Result<(), String> {
    let trimmed = method.trim();
    if trimmed.is_empty() {
        return Err("'method' cannot be empty".to_string());
    }
    // `method` becomes a URL path segment, so surrounding whitespace is an
    // error rather than something to silently strip.
    if trimmed != method {
        return Err(format!(
            "'method' must not contain surrounding whitespace, got {method:?}"
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "'method' must be a Bot API method name such as sendMessage, got {trimmed:?}"
        ));
    }
    if !trimmed.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err(format!("'method' must be camelCase, got {trimmed:?}"));
    }
    if FORBIDDEN_METHODS.contains(&trimmed) {
        return Err(format!(
            "'{trimmed}' is owned by this plugin and cannot be called through the passthrough: \
             the live poller owns the getUpdates offset, and a second consumer makes Telegram \
             answer 409 while dropping messages. Use telegram_get_updates instead."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn accepts_method_names() {
        for method in ["sendMessage", "getChat", "setMyName", "unknownFutureMethod"] {
            assert!(validate_method(method).is_ok(), "{method}");
        }
    }

    #[test]
    fn rejects_path_injection_and_lowercase() {
        assert!(validate_method("sendMessage/../../etc").is_err());
        assert!(validate_method("sendMessage?x=1").is_err());
        assert!(validate_method("send message").is_err());
        assert!(validate_method("").is_err());
        assert!(validate_method("sendMessage\n").is_err());
        assert!(validate_method("SendMessage").is_err());
    }

    #[test]
    fn refuses_the_methods_the_plugin_owns() {
        let err = validate_method("getUpdates").unwrap_err();
        assert!(err.contains("owned by this plugin"), "{err}");
        // Everything else is allowed: the guard is about path shape and the one
        // method this plugin already owns.
        for method in ["logOut", "close", "setWebhook", "deleteWebhook"] {
            assert!(validate_method(method).is_ok(), "{method}");
        }
    }

    #[test]
    fn builds_requests() {
        let built = build(&args(&[
            ("method", json!("sendMessage")),
            ("params", json!({ "chat_id": 1, "text": "hi" })),
        ]))
        .unwrap();
        assert_eq!(built.method, "sendMessage");
        assert_eq!(built.params["text"], "hi");

        // No params is an empty object, not an error.
        let built = build(&args(&[("method", json!("getMe"))])).unwrap();
        assert_eq!(built.params, json!({}));

        assert!(
            build(&args(&[
                ("method", json!("sendMessage")),
                ("params", json!("x"))
            ]))
            .is_err()
        );
        assert!(build(&args(&[])).is_err());
    }
}
