use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use neuxdot_core::db::schema::ModelConfigInput;
use serde::Deserialize;
use serde_json::json;

use crate::middleware::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(untagged)]
pub enum ProviderModelPayload {
    Simple(String),
    Detailed {
        model_id: String,
        name: Option<String>,
        config_json: Option<String>,
        #[serde(default)]
        params_json: Option<String>,
        #[serde(default)]
        context_window: Option<i64>,
    },
}

impl ProviderModelPayload {
    pub fn into_model_config(self) -> ModelConfigInput {
        match self {
            Self::Simple(mid) => ModelConfigInput {
                model_id: mid,
                name: None,
                config_json: None,
                params_json: None,
                context_window: None,
            },
            Self::Detailed {
                model_id,
                name,
                config_json,
                params_json,
                context_window,
            } => ModelConfigInput {
                model_id,
                name,
                config_json,
                params_json,
                context_window,
            },
        }
    }
}

#[derive(Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Option<Vec<ProviderModelPayload>>,
}

#[derive(Deserialize)]
pub struct UpdateProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Option<Vec<ProviderModelPayload>>,
}

#[derive(Deserialize)]
pub struct FetchModelsRequest {
    pub base_url: String,
    pub api_key: Option<String>,
}

pub async fn list_providers_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let providers = state
        .db
        .list_providers_with_models(&auth.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(providers))
}

pub async fn create_provider_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateProviderRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let provider = state
        .db
        .create_provider(
            &auth.account_id,
            &payload.name,
            &payload.base_url,
            payload.api_key.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    if let Some(model_list) = payload.models {
        let configs: Vec<ModelConfigInput> = model_list
            .into_iter()
            .map(|m| m.into_model_config())
            .collect();
        let _ = state.db.set_provider_models(&provider.id, &configs).await;
    }

    let result = state
        .db
        .get_provider_with_models(&provider.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Provider not found" }))))?;

    Ok((StatusCode::CREATED, Json(result)))
}

pub async fn update_provider_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateProviderRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_provider(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Provider not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let _updated = state
        .db
        .update_provider(
            &id,
            &payload.name,
            &payload.base_url,
            payload.api_key.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    if let Some(model_list) = payload.models {
        let configs: Vec<ModelConfigInput> = model_list
            .into_iter()
            .map(|m| m.into_model_config())
            .collect();
        let _ = state.db.set_provider_models(&id, &configs).await;
    }

    let result = state
        .db
        .get_provider_with_models(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Provider not found" }))))?;

    Ok(Json(result))
}

pub async fn delete_provider_handler(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let existing = state
        .db
        .get_provider(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({ "error": "Provider not found" }))))?;

    if existing.account_id != auth.account_id {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied" }))));
    }

    let deleted = state
        .db
        .delete_provider(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({ "deleted": deleted })))
}

pub async fn fetch_models_handler(
    _auth: AuthUser,
    Json(payload): Json<FetchModelsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let base_url = payload.base_url.trim().trim_end_matches('/');
    let url = format!("{}/models", base_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let mut req = client.get(&url);
    if let Some(key) = payload.api_key.as_deref() {
        if !key.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {}", key.trim()));
        }
    }

    let resp = req.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Failed to reach endpoint {}: {}", url, e) })),
        )
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Provider returned HTTP {}: {}", status, body) })),
        ));
    }

    let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Failed to parse JSON response: {}", e) })),
        )
    })?;

    let mut models = Vec::new();

    // OpenAI format: { "data": [ { "id": "model_id" } ] }
    if let Some(data) = resp_json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                models.push(id.to_string());
            }
        }
    }
    // Ollama format: { "models": [ { "name": "model_name" } ] }
    else if let Some(ollama_models) = resp_json.get("models").and_then(|m| m.as_array()) {
        for item in ollama_models {
            if let Some(name) = item.get("name").or_else(|| item.get("model")).and_then(|n| n.as_str()) {
                models.push(name.to_string());
            }
        }
    }

    models.sort();
    models.dedup();

    Ok(Json(json!({ "models": models })))
}
