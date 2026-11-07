use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tracing::info;

use neuxdot_core::db::schema::Extension;
use neuxdot_core::runtime::WasmExtensionRunner;

use crate::middleware::AuthUser;
use crate::state::AppState;

fn get_storage_base_dir() -> PathBuf {
    let dir = PathBuf::from("data/extensions");
    let _ = fs::create_dir_all(&dir);
    dir
}

#[derive(Serialize)]
pub struct ExtensionDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub wasm_hash: String,
    pub manifest: Value,
    pub is_active: bool,
    pub storage_bytes: i64,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct PatchExtensionPayload {
    pub is_active: Option<bool>,
}

#[derive(Deserialize)]
pub struct InstallUrlPayload {
    pub url: String,
}

#[derive(Serialize)]
pub struct StorageInfo {
    pub extension_id: String,
    pub wasm_hash: String,
    pub total_storage_bytes: i64,
    pub files: Vec<FileInfo>,
    pub kv_count: usize,
}

#[derive(Serialize)]
pub struct FileInfo {
    pub name: String,
    pub size_bytes: u64,
}

// GET /api/extensions
pub async fn list_extensions_handler(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let exts = state
        .db
        .list_extensions(&auth.account_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let dtos: Vec<ExtensionDto> = exts
        .into_iter()
        .map(|ext| {
            let manifest: Value = serde_json::from_str(&ext.manifest_json).unwrap_or(json!({}));
            ExtensionDto {
                id: ext.id,
                name: ext.name,
                version: ext.version,
                description: ext.description,
                wasm_hash: ext.wasm_hash,
                manifest,
                is_active: ext.is_active,
                storage_bytes: ext.storage_bytes,
                created_at: ext.created_at,
            }
        })
        .collect();

    Ok(Json(dtos))
}

// POST /api/extensions/upload (Multipart upload of .wasm file)
pub async fn upload_extension_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut wasm_bytes: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "wasm" {
            let data = field.bytes().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Failed to read multipart stream: {}", e),
                )
            })?;
            wasm_bytes = Some(data.to_vec());
            break;
        }
    }

    let bytes = wasm_bytes.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "No wasm file provided in multipart upload (expected field 'file')".to_string(),
        )
    })?;

    process_and_save_wasm(state, &auth.account_id, bytes).await
}

// POST /api/extensions/inspect (Inspect WASM manifest and check existing version before install)
pub async fn inspect_extension_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut wasm_bytes = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || name == "wasm" {
            let data = field.bytes().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Failed to read multipart stream: {}", e),
                )
            })?;
            wasm_bytes = Some(data.to_vec());
            break;
        }
    }

    let bytes = wasm_bytes.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "No wasm file provided in multipart upload (expected field 'file')".to_string(),
        )
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let wasm_hash = hex::encode(hasher.finalize());

    let temp_id = format!("inspect_{}", &wasm_hash[..8]);
    let base_storage_dir = get_storage_base_dir();

    let runner = WasmExtensionRunner::new(&bytes, temp_id, &base_storage_dir, state.db.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid WASM binary: {}", e)))?;

    let manifest_str = runner
        .get_manifest()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Failed to get manifest from WASM: {}", e)))?;

    let manifest_json: Value = serde_json::from_str(&manifest_str).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("WASM manifest is not valid JSON: {}", e),
        )
    })?;

    let id = manifest_json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(&wasm_hash[..12])
        .to_string();

    let name = manifest_json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("WASM Extension")
        .to_string();

    let version = manifest_json
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0.0")
        .to_string();

    let description = manifest_json
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let existing_ext = state
        .db
        .get_extension(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing_dto = existing_ext.map(|e| {
        let manifest: Value = serde_json::from_str(&e.manifest_json).unwrap_or(Value::Null);
        ExtensionDto {
            id: e.id,
            name: e.name,
            version: e.version,
            description: e.description,
            wasm_hash: e.wasm_hash,
            manifest,
            is_active: e.is_active,
            storage_bytes: e.storage_bytes,
            created_at: e.created_at,
        }
    });

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "version": version,
        "description": description,
        "wasm_hash": wasm_hash,
        "manifest": manifest_json,
        "existing": existing_dto
    })))
}

// POST /api/extensions/url (Download .wasm from URL)
pub async fn install_url_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(payload): Json<InstallUrlPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = client
        .get(&payload.url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Download failed: {}", e)))?;

    if !res.status().is_success() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Server returned HTTP {}", res.status()),
        ));
    }

    let bytes = res
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Failed to read response bytes: {}", e)))?
        .to_vec();

    process_and_save_wasm(state, &auth.account_id, bytes).await
}

async fn process_and_save_wasm(
    state: AppState,
    account_id: &str,
    bytes: Vec<u8>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let wasm_hash = hex::encode(hasher.finalize());

    // Temporary ID for reading manifest
    let temp_id = format!("temp_{}", &wasm_hash[..8]);
    let base_storage_dir = get_storage_base_dir();

    let runner = WasmExtensionRunner::new(&bytes, temp_id, &base_storage_dir, state.db.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid WASM binary: {}", e)))?;

    let manifest_str = runner
        .get_manifest()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Failed to get manifest from WASM: {}", e)))?;

    let manifest_json: Value = serde_json::from_str(&manifest_str).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("WASM manifest is not valid JSON: {}", e),
        )
    })?;

    let id = manifest_json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(&wasm_hash[..12])
        .to_string();

    let name = manifest_json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("WASM Extension")
        .to_string();

    let version = manifest_json
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0.0")
        .to_string();

    let description = manifest_json
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Check if updating an existing extension with a prior hash
    let existing_ext = state
        .db
        .get_extension(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(existing) = existing_ext {
        if existing.wasm_hash != wasm_hash {
            let old_dir = base_storage_dir.join(&existing.wasm_hash);
            let new_dir = base_storage_dir.join(&wasm_hash);
            if old_dir.exists() && old_dir.is_dir() {
                info!(
                    "Migrating extension '{}' storage from old hash ({}) to new hash ({})",
                    name, existing.wasm_hash, wasm_hash
                );
                let _ = std::fs::create_dir_all(&new_dir);
                if let Ok(entries) = std::fs::read_dir(&old_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(file_name) = path.file_name() {
                            let target = new_dir.join(file_name);
                            let _ = std::fs::copy(&path, &target);
                        }
                    }
                }
                // Clean up old hash folder completely
                let _ = std::fs::remove_dir_all(&old_dir);
                info!("Cleaned up old extension storage folder {}", old_dir.display());
            }
        }
    }

    let storage_bytes = runner.calculate_storage_usage().await;

    let ext = Extension {
        id: id.clone(),
        account_id: account_id.to_string(),
        name: name.clone(),
        version: version.clone(),
        description: description.clone(),
        wasm_hash: wasm_hash.clone(),
        wasm_binary: bytes,
        manifest_json: manifest_str,
        is_active: true,
        storage_bytes,
        created_at: None,
    };

    state
        .db
        .upsert_extension(&ext)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save extension: {}", e)))?;

    info!("Installed WASM extension '{}' (hash: {})", name, wasm_hash);

    let dto = ExtensionDto {
        id,
        name,
        version,
        description,
        wasm_hash,
        manifest: manifest_json,
        is_active: true,
        storage_bytes,
        created_at: None,
    };

    Ok((StatusCode::CREATED, Json(dto)))
}

// PATCH /api/extensions/:id (Toggle is_active)
pub async fn patch_extension_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<PatchExtensionPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ext_id = id.strip_prefix("wasm:").unwrap_or(&id);
    if let Some(active) = payload.is_active {
        let updated = state
            .db
            .set_extension_active(ext_id, active)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !updated {
            return Err((StatusCode::NOT_FOUND, "Extension not found".to_string()));
        }
    }

    Ok(Json(json!({ "success": true })))
}

// DELETE /api/extensions/:id (Uninstall)
pub async fn delete_extension_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ext_id = id.strip_prefix("wasm:").unwrap_or(&id);
    let ext = state
        .db
        .get_extension(ext_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(ext) = ext {
        // 1. Delete on-disk storage directory
        let dir = get_storage_base_dir().join(&ext.wasm_hash);
        if dir.exists() {
            let _ = fs::remove_dir_all(&dir);
        }

        // 2. Delete from DB (cascades to extension_kv)
        state
            .db
            .delete_extension(ext_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        info!("Uninstalled extension {}", ext_id);
        Ok(Json(json!({ "success": true })))
    } else {
        Err((StatusCode::NOT_FOUND, "Extension not found".to_string()))
    }
}

// GET /api/extensions/:id/kv (List KV entries)
pub async fn list_extension_kv_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ext_id = id.strip_prefix("wasm:").unwrap_or(&id);
    let items = state
        .db
        .list_extension_kv(ext_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(items))
}

// DELETE /api/extensions/:id/kv/:key
pub async fn delete_extension_kv_key_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ext_id = id.strip_prefix("wasm:").unwrap_or(&id);
    let deleted = state
        .db
        .delete_extension_kv(ext_id, &key)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": deleted })))
}

// GET /api/extensions/:id/storage
pub async fn get_extension_storage_handler(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ext_id = id.strip_prefix("wasm:").unwrap_or(&id);
    let ext = state
        .db
        .get_extension(ext_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Extension not found".to_string()))?;

    let dir = get_storage_base_dir().join(&ext.wasm_hash);
    let mut files = Vec::new();
    let mut total_disk_bytes = 0u64;

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total_disk_bytes += meta.len();
                    files.push(FileInfo {
                        name: entry.file_name().to_string_lossy().to_string(),
                        size_bytes: meta.len(),
                    });
                }
            }
        }
    }

    let kv_items = state
        .db
        .list_extension_kv(ext_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let kv_bytes: usize = kv_items.iter().map(|item| item.key.len() + item.value.len()).sum();
    let total_bytes = total_disk_bytes as i64 + kv_bytes as i64;

    // Update storage_bytes in database
    let _ = state.db.update_extension_storage(ext_id, total_bytes).await;

    Ok(Json(StorageInfo {
        extension_id: ext_id.to_string(),
        wasm_hash: ext.wasm_hash,
        total_storage_bytes: total_bytes,
        files,
        kv_count: kv_items.len(),
    }))
}

