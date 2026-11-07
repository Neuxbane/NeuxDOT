pub mod client;
pub mod custom_tools;
pub mod http;
pub mod stdio;

pub use client::*;
pub use custom_tools::*;
pub use http::*;
pub use stdio::*;

/// Host-stamped tool-argument keys.
///
/// These carry *trusted* caller identity from the host into a plugin. They are
/// underscore-prefixed so they never collide with a plugin's declared schema and
/// are never presented to the model. Defined once so the HTTP proxy and the
/// agent executor stamp exactly the same keys.
pub const SENDER_ACCOUNT_ID_KEY: &str = "_sender_account_id";
pub const SENDER_USERNAME_KEY: &str = "_sender_username";

/// Stamp caller identity onto `tools/call` arguments.
///
/// No-op for other methods or when `arguments` is not an object. The caller
/// cannot forge these because the host overwrites them unconditionally.
pub fn stamp_sender(
    method: &str,
    params: Option<serde_json::Value>,
    account_id: &str,
    username: &str,
) -> Option<serde_json::Value> {
    let mut params = params?;
    if method != "tools/call" {
        return Some(params);
    }
    if let Some(obj) = params.as_object_mut() {
        let mut args = obj
            .get("arguments")
            .and_then(|a| a.as_object())
            .cloned()
            .unwrap_or_default();
        args.insert(
            SENDER_ACCOUNT_ID_KEY.to_string(),
            serde_json::Value::String(account_id.to_string()),
        );
        args.insert(
            SENDER_USERNAME_KEY.to_string(),
            serde_json::Value::String(username.to_string()),
        );
        obj.insert("arguments".to_string(), serde_json::Value::Object(args));
    }
    Some(params)
}
