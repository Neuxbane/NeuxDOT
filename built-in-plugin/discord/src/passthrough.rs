//! `discord_api`: one guarded passthrough for endpoints the table does not name.
//!
//! The table in `tools.rs` covers what a bot actually does, but Discord's API is
//! larger, and an uncovered endpoint (auto-moderation create, soundboard, stage
//! instances) must not require a rebuild. The guard is deliberately about
//! *shape*, not about policy: the bot's own Discord role remains the authority,
//! exactly as it does for the typed tools.
//!
//! What is rejected is anything that cannot be expressed as a Discord resource
//! path, plus the credential endpoints — asking the plugin to talk to
//! `/oauth2/token` or `/auth/*` with a bot token is always a mistake.

use serde_json::{Map, Value};

use crate::tools::Verb;

/// Top-level resource collections a call may address.
const ALLOWED_ROOTS: &[&str] = &[
    "applications",
    "channels",
    "guilds",
    "interactions",
    "invites",
    "stage-instances",
    "stickers",
    "users",
    "voice",
    "webhooks",
];

/// Paths that exist but must never be reachable from a plugin tool.
const FORBIDDEN_ROOTS: &[&str] = &["auth", "oauth2", "login"];

pub struct Request {
    pub verb: Verb,
    /// Path plus query string, without the API base.
    pub path: String,
    pub body: Option<Value>,
}

/// Validate and assemble one passthrough request.
pub fn build(args: &Map<String, Value>) -> Result<Request, String> {
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .map(|m| m.trim().to_ascii_uppercase())
        .ok_or_else(|| "'method' is required".to_string())?;
    let verb = match method.as_str() {
        "GET" => Verb::Get,
        "POST" => Verb::Post,
        "PATCH" => Verb::Patch,
        "PUT" => Verb::Put,
        "DELETE" => Verb::Delete,
        other => {
            return Err(format!(
                "'method' must be GET, POST, PATCH, PUT or DELETE, got {other:?}"
            ));
        }
    };

    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| "'path' is required".to_string())?;
    let path = sanitize_path(raw)?;

    let body = match args.get("body") {
        Some(Value::Null) | None => None,
        Some(value @ Value::Object(_)) => Some(value.clone()),
        Some(_) => return Err("'body' must be a JSON object".to_string()),
    };
    if verb == Verb::Get && body.is_some() {
        return Err("a GET request cannot carry a body".to_string());
    }
    if matches!(verb, Verb::Post | Verb::Patch | Verb::Put) && body.is_none() {
        return Err(format!("{} requires a 'body'", verb.as_str()));
    }

    Ok(Request { verb, path, body })
}

/// Reject anything that is not a relative Discord resource path.
pub fn sanitize_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("'path' cannot be empty".to_string());
    }
    if raw.contains("://") {
        return Err("'path' is relative to the Discord API base, not a full URL".to_string());
    }
    if raw.contains("..") {
        return Err("'path' cannot contain '..'".to_string());
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("'path' cannot contain whitespace or control characters".to_string());
    }

    let trimmed = raw.trim_start_matches('/');
    let (bare, query) = match trimmed.split_once('?') {
        Some((bare, query)) => (bare, Some(query)),
        None => (trimmed, None),
    };
    if bare.is_empty() {
        return Err("'path' must name a resource, e.g. guilds/123/channels".to_string());
    }

    let first = bare
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if FORBIDDEN_ROOTS.contains(&first.as_str()) {
        return Err(format!(
            "'{first}' is a credential endpoint and cannot be reached from a tool"
        ));
    }
    if !ALLOWED_ROOTS.contains(&first.as_str()) {
        return Err(format!(
            "'{first}' is not a known Discord resource; expected one of: {}",
            ALLOWED_ROOTS.join(", ")
        ));
    }

    Ok(match query {
        Some(query) if !query.is_empty() => format!("{bare}?{query}"),
        _ => bare.to_string(),
    })
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
    fn accepts_resource_paths_and_normalises_them() {
        assert_eq!(
            sanitize_path("/guilds/123/channels").unwrap(),
            "guilds/123/channels"
        );
        assert_eq!(
            sanitize_path("guilds/123/members?limit=10").unwrap(),
            "guilds/123/members?limit=10"
        );
        assert_eq!(
            sanitize_path("guilds/1/auto-moderation/rules").unwrap(),
            "guilds/1/auto-moderation/rules"
        );
    }

    #[test]
    fn rejects_urls_traversal_whitespace_and_unknown_roots() {
        assert!(sanitize_path("https://evil.example/guilds/1").is_err());
        assert!(sanitize_path("guilds/../../etc").is_err());
        assert!(sanitize_path("guilds 1").is_err());
        assert!(sanitize_path("").is_err());
        assert!(sanitize_path("/").is_err());
        assert!(sanitize_path("guilds/1\n").is_err());
        assert!(sanitize_path("admin/guilds").is_err());
    }

    #[test]
    fn refuses_credential_endpoints() {
        for path in ["auth/login", "oauth2/token", "oauth2/authorize"] {
            let err = sanitize_path(path).unwrap_err();
            assert!(err.contains("credential endpoint"), "{path}: {err}");
        }
    }

    #[test]
    fn builds_requests_and_enforces_the_verb_body_contract() {
        let built = build(&args(&[
            ("method", json!("patch")),
            ("path", json!("/guilds/1")),
            ("body", json!({ "name": "New" })),
        ]))
        .unwrap();
        assert_eq!(built.verb, Verb::Patch);
        assert_eq!(built.path, "guilds/1");
        assert_eq!(built.body, Some(json!({ "name": "New" })));

        // A GET needs no body and must not carry one.
        assert!(
            build(&args(&[
                ("method", json!("GET")),
                ("path", json!("users/@me"))
            ]))
            .is_ok()
        );
        assert!(
            build(&args(&[
                ("method", json!("GET")),
                ("path", json!("users/@me")),
                ("body", json!({ "a": 1 })),
            ]))
            .is_err()
        );

        // A write needs a body.
        assert!(
            build(&args(&[
                ("method", json!("POST")),
                ("path", json!("guilds/1/channels"))
            ]))
            .is_err()
        );

        assert!(
            build(&args(&[
                ("method", json!("TRACE")),
                ("path", json!("guilds/1"))
            ]))
            .is_err()
        );
        assert!(build(&args(&[("method", json!("GET"))])).is_err());
        assert!(
            build(&args(&[
                ("method", json!("POST")),
                ("path", json!("guilds/1/channels")),
                ("body", json!("not an object")),
            ]))
            .is_err()
        );
    }
}
