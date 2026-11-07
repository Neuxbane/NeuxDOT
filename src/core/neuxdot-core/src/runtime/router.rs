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

            // Construct payload: base fields + optional config_json overrides
            let mut payload = json!({
                "model": target.model_id,
                "messages": messages,
            });

            if let Some(ref t) = tools {
                payload["tools"] = t.clone();
            }

            // Merge provider/model config_json (e.g. temperature, thinking, max_tokens, etc.)
            // Exclude non-completion metadata fields like context_window and name
            if let Some(ref cfg_str) = target.config_json {
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
