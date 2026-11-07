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
    #[error("Provider error ({status}): {body}")]
    ProviderError { status: u16, body: String },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
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

            let endpoint = format!(
                "{}/chat/completions",
                target.base_url.trim_end_matches('/')
            );

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
                    } else if status.as_u16() == 429 || status.is_server_error() {
                        let text = res.text().await.unwrap_or_default();
                        warn!(
                            "Target '{} ({})' failed with status {}, falling back...",
                            target.provider_name, target.model_id, status
                        );
                        last_err = Some(RouterError::ProviderError {
                            status: status.as_u16(),
                            body: text,
                        });
                        continue;
                    } else {
                        // Client error (e.g. 400 Bad Request) - return directly
                        let text = res.text().await.unwrap_or_default();
                        return Err(RouterError::ProviderError {
                            status: status.as_u16(),
                            body: text,
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        "Target '{} ({})' network error: {}, falling back...",
                        target.provider_name, target.model_id, e
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

    fn create_dummy_target(params_json: Option<&str>, config_json: Option<&str>) -> ActiveModelTarget {
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
            Some(r#"{"supports_thinking": true, "context_window": 128000, "icon": "robot", "name": "Sonnet"}"#),
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
            Some(r#"{"temperature": 0.5, "context_window": 64000, "name": "Legacy", "max_tokens": 1000}"#),
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
}
