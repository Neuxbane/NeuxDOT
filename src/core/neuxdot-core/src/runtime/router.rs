use rand::seq::SliceRandom;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

use crate::db::schema::ActiveModelTarget;

#[derive(Error, Debug)]
pub enum RouterError {
    #[error("No models available")]
    NoProviders,
    #[error("All models failed")]
    AllProvidersFailed,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Provider '{provider}' ({model}) failed on POST {endpoint} with HTTP {status}{status_text}: {body}")]
    ProviderError {
        provider: String,
        model: String,
        endpoint: String,
        status: u16,
        status_text: String,
        body: String,
    },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

fn format_provider_error_body(status: u16, endpoint: &str, raw_body: &str) -> String {
    let trimmed = raw_body.trim();
    if trimmed.is_empty() {
        if status == 404 || status == 405 {
            let tip = if !endpoint.contains("/v1/") && !endpoint.ends_with("/v1") {
                " (Tip: HTTP 405/404 Method Not Allowed or Not Found typically indicates an incorrect Base URL. Please verify if the provider requires a '/v1' path prefix, e.g. 'https://.../v1')"
            } else {
                " (Tip: HTTP 405/404 Method Not Allowed or Not Found typically indicates an incorrect endpoint URL.)"
            };
            return format!("<empty response body>{}", tip);
        }
        return "<empty response body>".to_string();
    }

    // Try parsing as JSON error
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        if let Some(err_obj) = val.get("error") {
            if let Some(msg) = err_obj.as_str() {
                return format!("error: {}", msg);
            }
            if let Some(obj) = err_obj.as_object() {
                let msg = obj.get("message").and_then(|m| m.as_str()).unwrap_or("");
                let code = obj
                    .get("code")
                    .and_then(|c| c.as_str().or_else(|| c.as_i64().map(|_| "")));
                let typ = obj.get("type").and_then(|t| t.as_str());

                let mut details = Vec::new();
                if let Some(c) = code {
                    if !c.is_empty() {
                        details.push(format!("code: {}", c));
                    }
                }
                if let Some(t) = typ {
                    details.push(format!("type: {}", t));
                }

                if !msg.is_empty() {
                    if details.is_empty() {
                        return format!("error: {}", msg);
                    } else {
                        return format!("error: {} ({})", msg, details.join(", "));
                    }
                }
            }
        } else if let Some(msg) = val.get("message").and_then(|m| m.as_str()) {
            return format!("message: {}", msg);
        }
        let json_str = val.to_string();
        if json_str.len() > 500 {
            return format!("{}... [truncated]", &json_str[..500]);
        }
        return json_str;
    }

    // Check if HTML
    let lower = trimmed.to_lowercase();
    if lower.starts_with("<!doctype") || lower.starts_with("<html") || lower.contains("<head>") {
        if let Some(start) = lower.find("<title>") {
            if let Some(end) = lower[start..].find("</title>") {
                let title = &trimmed[start + 7..start + end];
                return format!("HTML response (<title>: {})", title.trim());
            }
        }
        return "HTML error page returned by provider".to_string();
    }

    if trimmed.len() > 500 {
        format!("{}... [truncated]", &trimmed[..500])
    } else {
        trimmed.to_string()
    }
}

pub struct ProviderRouter {
    targets: Vec<ActiveModelTarget>,
    strategy: String,
    round_robin_counter: Arc<AtomicUsize>,
    http_client: Client,
}

impl ProviderRouter {
    pub fn new(targets: Vec<ActiveModelTarget>, strategy: &str) -> Self {
        Self {
            targets,
            strategy: strategy.to_lowercase(),
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
            http_client: Client::builder().build().unwrap_or_default(),
        }
    }

    pub fn pick_candidate_order(&self) -> Vec<&ActiveModelTarget> {
        if self.targets.is_empty() {
            return vec![];
        }

        match self.strategy.as_str() {
            "round-robin" | "round_robin" => {
                let count = self.targets.len();
                let start_idx = self.round_robin_counter.fetch_add(1, Ordering::SeqCst) % count;
                let mut ordered = Vec::with_capacity(count);
                for i in 0..count {
                    ordered.push(&self.targets[(start_idx + i) % count]);
                }
                ordered
            }
            "random" => {
                let mut rng = rand::thread_rng();
                let mut indices: Vec<usize> = (0..self.targets.len()).collect();
                indices.shuffle(&mut rng);
                indices.into_iter().map(|i| &self.targets[i]).collect()
            }
            _ => {
                // Default: Fallback (in defined order)
                self.targets.iter().collect()
            }
        }
    }

    pub fn build_payload(
        target: &ActiveModelTarget,
        messages: &[Value],
        tools: Option<&Value>,
    ) -> Value {
        let mut payload = json!({
            "model": target.model_id,
            "messages": messages,
        });

        if let Some(t) = tools {
            payload["tools"] = t.clone();
        }

        // Merge provider/model parameters:
        // Prefer params_json verbatim (completion hyperparameters only, no name filtering).
        // Fall back to config_json if params_json is not present (for backwards compatibility).
        if let Some(ref params_str) = target.params_json {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(params_str) {
                if let Some(target_obj) = payload.as_object_mut() {
                    for (k, v) in map {
                        target_obj.insert(k, v);
                    }
                }
            }
        } else if let Some(ref cfg_str) = target.config_json {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(cfg_str) {
                if let Some(target_obj) = payload.as_object_mut() {
                    for (k, v) in map {
                        if k != "context_window" && k != "name" {
                            target_obj.insert(k, v);
                        }
                    }
                }
            }
        }

        payload
    }

    pub async fn execute_chat_completion(
        &self,
        messages: Vec<Value>,
        tools: Option<Value>,
        on_target_selected: impl Fn(&ActiveModelTarget),
    ) -> Result<(Value, ActiveModelTarget), RouterError> {
        let candidates = self.pick_candidate_order();
        if candidates.is_empty() {
            return Err(RouterError::NoProviders);
        }

        let mut last_err = None;

        for target in candidates {
            on_target_selected(target);

            let payload = Self::build_payload(target, &messages, tools.as_ref());

            let endpoint = format!("{}/chat/completions", target.base_url.trim_end_matches('/'));

            let mut req = self.http_client.post(&endpoint).json(&payload);
            if let Some(ref key) = target.api_key {
                if !key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {}", key));
                }
            }

            match req.send().await {
                Ok(res) => {
                    let status = res.status();
                    if status.is_success() {
                        let body: Value = res.json().await?;
                        return Ok((body, target.clone()));
                    } else {
                        let status_text = status
                            .canonical_reason()
                            .map(|r| format!(" {}", r))
                            .unwrap_or_default();
                        let text = res.text().await.unwrap_or_default();
                        let formatted_body =
                            format_provider_error_body(status.as_u16(), &endpoint, &text);

                        let err = RouterError::ProviderError {
                            provider: target.provider_name.clone(),
                            model: target.model_id.clone(),
                            endpoint: endpoint.clone(),
                            status: status.as_u16(),
                            status_text: status_text.clone(),
                            body: formatted_body,
                        };

                        warn!(
                            "Target '{} ({})' failed on POST {} with HTTP {}{}: {}, falling back...",
                            target.provider_name, target.model_id, endpoint, status, status_text, text
                        );

                        last_err = Some(err);
                        continue;
                    }
                }
                Err(e) => {
                    warn!(
                        "Target '{} ({})' network error on POST {}: {}, falling back...",
                        target.provider_name, target.model_id, endpoint, e
                    );
                    last_err = Some(RouterError::Network(e));
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or(RouterError::AllProvidersFailed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_dummy_target(
        params_json: Option<&str>,
        config_json: Option<&str>,
    ) -> ActiveModelTarget {
        ActiveModelTarget {
            id: "pm_test".to_string(),
            provider_id: "p_test".to_string(),
            provider_name: "TestProvider".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: None,
            model_id: "test-model".to_string(),
            model_name: Some("Test Model".to_string()),
            config_json: config_json.map(|s| s.to_string()),
            params_json: params_json.map(|s| s.to_string()),
            context_window: Some(32000),
        }
    }

    #[test]
    fn test_router_payload_omits_metadata_and_merges_params_json_verbatim() {
        let target = create_dummy_target(
            Some(r#"{"temperature": 0.7, "max_tokens": 2048, "top_p": 0.95}"#),
            Some(
                r#"{"supports_thinking": true, "context_window": 128000, "icon": "robot", "name": "Sonnet"}"#,
            ),
        );
        let messages = vec![json!({"role": "user", "content": "hi"})];
        let payload = ProviderRouter::build_payload(&target, &messages, None);

        assert_eq!(payload["model"], "test-model");
        assert_eq!(payload["temperature"], 0.7);
        assert_eq!(payload["max_tokens"], 2048);
        assert_eq!(payload["top_p"], 0.95);
        assert!(payload.get("supports_thinking").is_none());
        assert!(payload.get("context_window").is_none());
        assert!(payload.get("icon").is_none());
        assert!(payload.get("name").is_none());
    }

    #[test]
    fn test_router_payload_fallback_to_config_json_when_params_json_is_none() {
        let target = create_dummy_target(
            None,
            Some(
                r#"{"temperature": 0.5, "context_window": 64000, "name": "Legacy", "max_tokens": 1000}"#,
            ),
        );
        let messages = vec![json!({"role": "user", "content": "hello"})];
        let payload = ProviderRouter::build_payload(&target, &messages, None);

        assert_eq!(payload["model"], "test-model");
        assert_eq!(payload["temperature"], 0.5);
        assert_eq!(payload["max_tokens"], 1000);
        assert!(payload.get("context_window").is_none());
        assert!(payload.get("name").is_none());
    }

    #[test]
    fn test_router_payload_unknown_metadata_key_not_forwarded() {
        let target = create_dummy_target(
            Some(r#"{"temperature": 0.0}"#),
            Some(r#"{"supports_thinking": true, "unknown_field_xyz": "value"}"#),
        );
        let messages = vec![json!({"role": "user", "content": "hello"})];
        let payload = ProviderRouter::build_payload(&target, &messages, None);

        assert_eq!(payload["temperature"], 0.0);
        assert!(payload.get("supports_thinking").is_none());
        assert!(payload.get("unknown_field_xyz").is_none());
    }

    #[test]
    fn test_format_provider_error_body_empty_405_adds_url_tip() {
        let endpoint = "https://kenari.id/chat/completions";
        let formatted = format_provider_error_body(405, endpoint, "");
        assert!(formatted.contains("<empty response body>"));
        assert!(formatted.contains("Tip: HTTP 405/404 Method Not Allowed or Not Found typically indicates an incorrect Base URL"));
        assert!(formatted.contains("'/v1'"));

        let err = RouterError::ProviderError {
            provider: "kenari".to_string(),
            model: "deepseek-v4-1-flash".to_string(),
            endpoint: endpoint.to_string(),
            status: 405,
            status_text: " Method Not Allowed".to_string(),
            body: formatted,
        };
        let msg = err.to_string();
        assert!(msg.contains("Provider 'kenari' (deepseek-v4-1-flash) failed on POST https://kenari.id/chat/completions with HTTP 405 Method Not Allowed:"));
        assert!(msg.contains("Tip:"));
    }

    #[test]
    fn test_format_provider_error_body_json() {
        let endpoint = "https://api.openai.com/v1/chat/completions";
        let raw = r#"{"error": {"message": "Invalid API key", "code": "invalid_api_key", "type": "auth_error"}}"#;
        let formatted = format_provider_error_body(401, endpoint, raw);
        assert_eq!(
            formatted,
            "error: Invalid API key (code: invalid_api_key, type: auth_error)"
        );
    }

    #[test]
    fn test_format_provider_error_body_html() {
        let endpoint = "https://example.com/chat/completions";
        let raw = r#"<!DOCTYPE html><html><head><title>405 Method Not Allowed</title></head><body><h1>405</h1></body></html>"#;
        let formatted = format_provider_error_body(405, endpoint, raw);
        assert_eq!(formatted, "HTML response (<title>: 405 Method Not Allowed)");
    }
}
