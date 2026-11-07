use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasmi::{Caller, Engine, Linker, Module, Store};

use crate::db::repos::Database;
use crate::runtime::executor::AgentExecutor;

fn run_async<F, R>(f: F) -> R
where
    F: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || handle.block_on(f))
        .join()
        .expect("async thread panicked")
}

pub struct HostContext {
    pub extension_id: String,
    pub wasm_hash: String,
    pub storage_dir: PathBuf,
    pub db: Arc<Database>,
    // Buffer for passing dynamic output from host to wasm or vice versa
    pub temp_buffer: Vec<u8>,
}

pub struct WasmExtensionRunner {
    engine: Engine,
    module: Module,
    extension_id: String,
    wasm_hash: String,
    storage_dir: PathBuf,
    db: Arc<Database>,
}

impl WasmExtensionRunner {
    pub fn new(
        wasm_bytes: &[u8],
        extension_id: String,
        base_storage_dir: &Path,
        db: Arc<Database>,
    ) -> Result<Self> {
        let mut hasher = Sha256::new();
        hasher.update(wasm_bytes);
        let wasm_hash = hex::encode(hasher.finalize());

        let storage_dir = base_storage_dir.join(&wasm_hash);
        fs::create_dir_all(&storage_dir)?;

        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;

        Ok(Self {
            engine,
            module,
            extension_id,
            wasm_hash,
            storage_dir,
            db,
        })
    }

    pub fn wasm_hash(&self) -> &str {
        &self.wasm_hash
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    /// Calculate total storage used on disk and in KV store
    pub async fn calculate_storage_usage(&self) -> i64 {
        let mut total = 0i64;

        // 1. Files on disk
        if let Ok(entries) = fs::read_dir(&self.storage_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len() as i64;
                }
            }
        }

        // 2. KV storage in DB
        if let Ok(kv_items) = self.db.list_extension_kv(&self.extension_id).await {
            for item in kv_items {
                total += (item.key.len() + item.value.len()) as i64;
            }
        }

        total
    }

    fn create_linker(&self) -> Result<Linker<HostContext>> {
        let mut linker = Linker::<HostContext>::new(&self.engine);

        // host_log(ptr, len)
        linker.func_wrap(
            "env",
            "host_log",
            |caller: Caller<'_, HostContext>, ptr: i32, len: i32| {
                if let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let mut buf = vec![0u8; len as usize];
                    if mem.read(&caller, ptr as usize, &mut buf).is_ok() {
                        let msg = String::from_utf8_lossy(&buf);
                        tracing::info!("[WASM Extension {}] {}", caller.data().extension_id, msg);
                    }
                }
            },
        )?;

        // host_kv_get(key_ptr, key_len, out_ptr, max_out) -> i32 (bytes written, or -1 on error/not found)
        linker.func_wrap(
            "env",
            "host_kv_get",
            |mut caller: Caller<'_, HostContext>,
             key_ptr: i32,
             key_len: i32,
             out_ptr: i32,
             max_out: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut key_bytes = vec![0u8; key_len as usize];
                if mem.read(&caller, key_ptr as usize, &mut key_bytes).is_err() {
                    return -1;
                }
                let key_str = match String::from_utf8(key_bytes) {
                    Ok(k) => k,
                    Err(_) => return -1,
                };

                let ext_id = caller.data().extension_id.clone();
                let db = caller.data().db.clone();

                let res = run_async(async move {
                    db.get_extension_kv(&ext_id, &key_str).await
                });

                match res {
                    Ok(Some(val)) => {
                        let val_bytes = val.as_bytes();
                        let write_len = val_bytes.len().min(max_out as usize);
                        if mem.write(&mut caller, out_ptr as usize, &val_bytes[..write_len]).is_ok() {
                            write_len as i32
                        } else {
                            -1
                        }
                    }
                    _ => -1,
                }
            },
        )?;

        // host_kv_set(key_ptr, key_len, val_ptr, val_len) -> i32 (0 on success, -1 on error)
        linker.func_wrap(
            "env",
            "host_kv_set",
            |caller: Caller<'_, HostContext>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32,
             val_len: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut key_bytes = vec![0u8; key_len as usize];
                let mut val_bytes = vec![0u8; val_len as usize];
                if mem.read(&caller, key_ptr as usize, &mut key_bytes).is_err()
                    || mem.read(&caller, val_ptr as usize, &mut val_bytes).is_err()
                {
                    return -1;
                }

                let (key_str, val_str) = match (String::from_utf8(key_bytes), String::from_utf8(val_bytes)) {
                    (Ok(k), Ok(v)) => (k, v),
                    _ => return -1,
                };

                let ext_id = caller.data().extension_id.clone();
                let db = caller.data().db.clone();

                let res = run_async(async move {
                    db.set_extension_kv(&ext_id, &key_str, &val_str).await
                });

                match res {
                    Ok(()) => 0,
                    Err(e) => {
                        tracing::error!("host_kv_set failed: {}", e);
                        -1
                    }
                }
            },
        )?;

        // host_kv_delete(key_ptr, key_len) -> i32
        linker.func_wrap(
            "env",
            "host_kv_delete",
            |caller: Caller<'_, HostContext>, key_ptr: i32, key_len: i32| -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut key_bytes = vec![0u8; key_len as usize];
                if mem.read(&caller, key_ptr as usize, &mut key_bytes).is_err() {
                    return -1;
                }
                let key_str = match String::from_utf8(key_bytes) {
                    Ok(k) => k,
                    Err(_) => return -1,
                };

                let ext_id = caller.data().extension_id.clone();
                let db = caller.data().db.clone();

                let res = run_async(async move {
                    db.delete_extension_kv(&ext_id, &key_str).await
                });

                if let Ok(deleted) = res {
                    if deleted {
                        0
                    } else {
                        1
                    }
                } else {
                    -1
                }
            },
        )?;

        // host_fs_write(path_ptr, path_len, data_ptr, data_len) -> i32 (0 success, -1 error)
        linker.func_wrap(
            "env",
            "host_fs_write",
            |caller: Caller<'_, HostContext>,
             path_ptr: i32,
             path_len: i32,
             data_ptr: i32,
             data_len: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut path_bytes = vec![0u8; path_len as usize];
                let mut data_bytes = vec![0u8; data_len as usize];
                if mem.read(&caller, path_ptr as usize, &mut path_bytes).is_err()
                    || mem.read(&caller, data_ptr as usize, &mut data_bytes).is_err()
                {
                    return -1;
                }

                let rel_path = match String::from_utf8(path_bytes) {
                    Ok(p) => p,
                    Err(_) => return -1,
                };

                // Security check: strictly disallow parent traversal
                let clean_name = Path::new(&rel_path).file_name();
                let target_path = match clean_name {
                    Some(name) => caller.data().storage_dir.join(name),
                    None => return -1,
                };

                if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&target_path) {
                    if std::io::Write::write_all(&mut file, &data_bytes).is_ok() {
                        0
                    } else {
                        -1
                    }
                } else {
                    -1
                }
            },
        )?;

        // host_fs_read(path_ptr, path_len, out_ptr, max_out) -> i32
        linker.func_wrap(
            "env",
            "host_fs_read",
            |mut caller: Caller<'_, HostContext>,
             path_ptr: i32,
             path_len: i32,
             out_ptr: i32,
             max_out: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut path_bytes = vec![0u8; path_len as usize];
                if mem.read(&caller, path_ptr as usize, &mut path_bytes).is_err() {
                    return -1;
                }

                let rel_path = match String::from_utf8(path_bytes) {
                    Ok(p) => p,
                    Err(_) => return -1,
                };

                let clean_name = Path::new(&rel_path).file_name();
                let target_path = match clean_name {
                    Some(name) => caller.data().storage_dir.join(name),
                    None => return -1,
                };

                match fs::read(target_path) {
                    Ok(content) => {
                        let write_len = content.len().min(max_out as usize);
                        if mem.write(&mut caller, out_ptr as usize, &content[..write_len]).is_ok() {
                            write_len as i32
                        } else {
                            -1
                        }
                    }
                    Err(_) => -1,
                }
            },
        )?;

        // host_agent_run(agent_id_ptr, agent_id_len, input_ptr, input_len, out_ptr, max_out) -> i32
        linker.func_wrap(
            "env",
            "host_agent_run",
            |mut caller: Caller<'_, HostContext>,
             agent_id_ptr: i32,
             agent_id_len: i32,
             input_ptr: i32,
             input_len: i32,
             out_ptr: i32,
             max_out: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut agent_id_bytes = vec![0u8; agent_id_len as usize];
                if mem.read(&caller, agent_id_ptr as usize, &mut agent_id_bytes).is_err() {
                    return -1;
                }
                let agent_id = match String::from_utf8(agent_id_bytes) {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let mut input_bytes = vec![0u8; input_len as usize];
                if mem.read(&caller, input_ptr as usize, &mut input_bytes).is_err() {
                    return -1;
                }
                let input_str = match String::from_utf8(input_bytes) {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let db = caller.data().db.clone();

                let res = run_async(async move {
                    let agent = match db.get_agent(&agent_id).await {
                        Ok(Some(a)) => a,
                        _ => return Err("Agent not found".to_string()),
                    };

                    if !agent.is_active {
                        return Ok(format!(
                            "Agent '{}' is currently inactive. Turn on the agent in the sidebar to interact.",
                            agent.name
                        ));
                    }

                    let models = db.get_agent_models(&agent.id).await.unwrap_or_default();
                    if models.is_empty() {
                        return Ok(format!(
                            "Agent '{}' has no provider models connected. Connect a model in Models & Routing to chat.",
                            agent.name
                        ));
                    }

                    let mcp_servers = db.get_agent_mcp_servers(&agent.id).await.unwrap_or_default();
                    let (event_tx, _) = tokio::sync::broadcast::channel(16);

                    let executor = AgentExecutor::new(
                        Arc::clone(&db),
                        agent,
                        models,
                        mcp_servers,
                        event_tx,
                    );

                    executor.run_turn(&input_str).await
                });

                let output_text = match res {
                    Ok(text) => text,
                    Err(e) => format!("Execution error: {}", e),
                };

                let out_bytes = output_text.as_bytes();
                let write_len = out_bytes.len().min(max_out as usize);
                if mem.write(&mut caller, out_ptr as usize, &out_bytes[..write_len]).is_ok() {
                    write_len as i32
                } else {
                    -1
                }
            },
        )?;

        // host_agent_run_background(agent_id_ptr, agent_id_len, input_ptr, input_len) -> i32
        linker.func_wrap(
            "env",
            "host_agent_run_background",
            |caller: Caller<'_, HostContext>,
             agent_id_ptr: i32,
             agent_id_len: i32,
             input_ptr: i32,
             input_len: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };

                let mut agent_id_bytes = vec![0u8; agent_id_len as usize];
                if mem.read(&caller, agent_id_ptr as usize, &mut agent_id_bytes).is_err() {
                    return -1;
                }
                let agent_id = match String::from_utf8(agent_id_bytes) {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let mut input_bytes = vec![0u8; input_len as usize];
                if mem.read(&caller, input_ptr as usize, &mut input_bytes).is_err() {
                    return -1;
                }
                let input_str = match String::from_utf8(input_bytes) {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let db = caller.data().db.clone();
                let ext_id = caller.data().extension_id.clone();

                tokio::spawn(async move {
                    let agent = match db.get_agent(&agent_id).await {
                        Ok(Some(a)) => a,
                        _ => return,
                    };

                    if !agent.is_active {
                        return;
                    }

                    let models = db.get_agent_models(&agent.id).await.unwrap_or_default();
                    if models.is_empty() {
                        return;
                    }

                    let mcp_servers = db.get_agent_mcp_servers(&agent.id).await.unwrap_or_default();
                    let (event_tx, _) = tokio::sync::broadcast::channel(16);

                    let executor = AgentExecutor::new(
                        Arc::clone(&db),
                        agent,
                        models,
                        mcp_servers,
                        event_tx,
                    );

                    match executor.run_turn(&input_str).await {
                        Ok(output_text) => {
                            let trimmed = output_text.trim();
                            if !trimmed.is_empty()
                                && !trimmed.contains("Output Text is not allowed")
                                && !trimmed.starts_with("Execution error:")
                            {
                                let time_str = chrono::Local::now()
                                    .format("%l:%M %p")
                                    .to_string()
                                    .trim()
                                    .to_string();
                                let key = format!("chat_history_{}", agent_id);
                                let history_json = db
                                    .get_extension_kv(&ext_id, &key)
                                    .await
                                    .unwrap_or(None)
                                    .unwrap_or_else(|| "[]".to_string());
                                let mut messages: Vec<serde_json::Value> =
                                    serde_json::from_str(&history_json).unwrap_or_default();
                                messages.push(serde_json::json!({
                                    "id": format!("ast_{}", messages.len() + 1),
                                    "role": "assistant",
                                    "text": trimmed,
                                    "timestamp": time_str
                                }));
                                if let Ok(updated_json) = serde_json::to_string(&messages) {
                                    let _ = db.set_extension_kv(&ext_id, &key, &updated_json).await;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Background turn for agent {} error: {}", agent_id, e);
                        }
                    }
                });

                0
            },
        )?;

        // host_get_time_str(out_ptr, max_out) -> i32 (bytes written, or -1)
        linker.func_wrap(
            "env",
            "host_get_time_str",
            |mut caller: Caller<'_, HostContext>, out_ptr: i32, max_out: i32| -> i32 {
                let now = chrono::Local::now();
                let time_str = now.format("%l:%M %p").to_string().trim().to_string();
                let bytes = time_str.as_bytes();
                let write_len = bytes.len().min(max_out as usize);
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                if mem.write(&mut caller, out_ptr as usize, &bytes[..write_len]).is_ok() {
                    write_len as i32
                } else {
                    -1
                }
            },
        )?;

        Ok(linker)
    }

    /// Extract manifest JSON from the WASM extension
    pub fn get_manifest(&self) -> Result<String> {
        let linker = self.create_linker()?;
        let ctx = HostContext {
            extension_id: self.extension_id.clone(),
            wasm_hash: self.wasm_hash.clone(),
            storage_dir: self.storage_dir.clone(),
            db: self.db.clone(),
            temp_buffer: Vec::new(),
        };

        let mut store = Store::new(&self.engine, ctx);
        let instance = linker.instantiate_and_start(&mut store, &self.module)?;

        let get_manifest_fn = instance
            .get_typed_func::<(), i64>(&store, "get_manifest")
            .map_err(|e| anyhow!("WASM missing export get_manifest(): {e}"))?;

        let res = get_manifest_fn.call(&mut store, ())?;
        let ptr = (res >> 32) as usize;
        let len = (res & 0xFFFFFFFF) as usize;

        let memory = instance
            .get_export(&store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| anyhow!("WASM missing memory export"))?;

        let mut buf = vec![0u8; len];
        memory.read(&store, ptr, &mut buf)?;

        let manifest = String::from_utf8(buf)?;
        Ok(manifest)
    }

    /// Execute an MCP JSON-RPC request inside the WASM module
    pub fn handle_mcp_request(&self, method: &str, params_json: &str) -> Result<String> {
        let linker = self.create_linker()?;
        let ctx = HostContext {
            extension_id: self.extension_id.clone(),
            wasm_hash: self.wasm_hash.clone(),
            storage_dir: self.storage_dir.clone(),
            db: self.db.clone(),
            temp_buffer: Vec::new(),
        };

        let mut store = Store::new(&self.engine, ctx);
        let instance = linker.instantiate_and_start(&mut store, &self.module)?;

        let alloc_fn = instance
            .get_typed_func::<i32, i32>(&store, "alloc")
            .map_err(|e| anyhow!("WASM missing alloc() function: {e}"))?;

        let memory = instance
            .get_export(&store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| anyhow!("WASM missing memory export"))?;

        // First try the pure MCP-Apps entrypoint `mcp_handle_request(method_ptr, method_len, params_ptr, params_len)`
        if let Ok(mcp_fn) = instance.get_typed_func::<(i32, i32, i32, i32), i64>(&store, "mcp_handle_request") {
            let method_bytes = method.as_bytes();
            let params_bytes = params_json.as_bytes();

            let method_ptr = alloc_fn.call(&mut store, method_bytes.len() as i32)?;
            memory.write(&mut store, method_ptr as usize, method_bytes)?;

            let params_ptr = alloc_fn.call(&mut store, params_bytes.len() as i32)?;
            memory.write(&mut store, params_ptr as usize, params_bytes)?;

            let res = mcp_fn.call(
                &mut store,
                (
                    method_ptr,
                    method_bytes.len() as i32,
                    params_ptr,
                    params_bytes.len() as i32,
                ),
            )?;

            let out_ptr = (res >> 32) as usize;
            let out_len = (res & 0xFFFFFFFF) as usize;

            let mut out_buf = vec![0u8; out_len];
            memory.read(&store, out_ptr, &mut out_buf)?;

            let out_str = String::from_utf8(out_buf)?;
            return Ok(out_str);
        }

        // Fallback for older WASM binaries exporting `execute_action`
        let mapped_action = match method {
            "tools/call" => "mcp_call_tool",
            "resources/read" => "mcp_read_resource",
            other => other,
        };

        let execute_fn = instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&store, "execute_action")
            .map_err(|e| anyhow!("WASM missing mcp_handle_request() or execute_action(): {e}"))?;

        let action_bytes = mapped_action.as_bytes();
        let payload_bytes = params_json.as_bytes();

        let action_ptr = alloc_fn.call(&mut store, action_bytes.len() as i32)?;
        memory.write(&mut store, action_ptr as usize, action_bytes)?;

        let payload_ptr = alloc_fn.call(&mut store, payload_bytes.len() as i32)?;
        memory.write(&mut store, payload_ptr as usize, payload_bytes)?;

        let res = execute_fn.call(
            &mut store,
            (
                action_ptr,
                action_bytes.len() as i32,
                payload_ptr,
                payload_bytes.len() as i32,
            ),
        )?;

        let out_ptr = (res >> 32) as usize;
        let out_len = (res & 0xFFFFFFFF) as usize;

        let mut out_buf = vec![0u8; out_len];
        memory.read(&store, out_ptr, &mut out_buf)?;

        let out_str = String::from_utf8(out_buf)?;
        Ok(out_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    #[tokio::test]
    async fn test_wasm_chat_extension_runner() {
        let wasm_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/chat-extension/target/wasm32-unknown-unknown/release/chat_extension.wasm"
        );
        if !std::path::Path::new(wasm_path).exists() {
            eprintln!("Skipping test: wasm file not found at {}", wasm_path);
            return;
        }

        let wasm_bytes = std::fs::read(wasm_path).expect("Read wasm file");
        let pool = init_db("sqlite::memory:").await.expect("Init in-memory DB");
        let db = Arc::new(Database::new(pool));

        let acc = db.create_account("root", "hash").await.expect("create acc");
        let ext = crate::db::schema::Extension {
            id: "community.agent-chat".to_string(),
            account_id: acc.id,
            name: "Agent Chat Console".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            wasm_hash: "mock_hash".to_string(),
            wasm_binary: wasm_bytes.clone(),
            manifest_json: "{}".to_string(),
            is_active: true,
            storage_bytes: 0,
            created_at: None,
        };
        db.upsert_extension(&ext).await.expect("upsert ext");

        let temp_dir = std::env::temp_dir().join(format!("neuxdot_test_ext_{}", uuid::Uuid::new_v4()));
        let runner = WasmExtensionRunner::new(
            &wasm_bytes,
            "community.agent-chat".to_string(),
            &temp_dir,
            db.clone(),
        )
        .expect("Create runner");

        // 1. Test get_manifest()
        let manifest = runner.get_manifest().expect("get_manifest");
        assert!(manifest.contains("Agent Chat Console"));
        assert!(manifest.contains("ui://chat-console"));
        assert!(!manifest.contains("ui_hooks"));

        // 2. Test resources/read for ui://chat-console
        let ui_res = runner
            .handle_mcp_request("resources/read", r#"{"uri":"ui://chat-console"}"#)
            .expect("resources/read");
        assert!(ui_res.contains("text/html;profile=mcp-app"));
        assert!(ui_res.contains("<!DOCTYPE html>"));

        // 3. Test tools/call chat_user_message (pure MCP-App user message)
        let user_tool_payload = serde_json::json!({
            "name": "chat_user_message",
            "arguments": {
                "agent_id": "agent_123",
                "message": "Hello from test"
            }
        });
        let res = runner
            .handle_mcp_request("tools/call", &user_tool_payload.to_string())
            .expect("chat_user_message");
        assert!(res.contains("Hello from test"));
        assert!(res.contains("user"));

        // 4. Test assistant sending message via chat_send tool
        let tool_payload = serde_json::json!({
            "name": "chat_send",
            "arguments": {
                "agent_id": "agent_123",
                "message": "Assistant reply from test"
            }
        });
        let tool_res = runner
            .handle_mcp_request("tools/call", &tool_payload.to_string())
            .expect("chat_send");
        assert!(tool_res.contains("Message posted to chat console"));

        // 5. Test tools/call chat_history (contains both user and assistant messages)
        let hist_payload = serde_json::json!({
            "name": "chat_history",
            "arguments": {
                "agent_id": "agent_123"
            }
        });
        let hist = runner
            .handle_mcp_request("tools/call", &hist_payload.to_string())
            .expect("chat_history");
        assert!(hist.contains("Hello from test"));
        assert!(hist.contains("Assistant reply from test"));
        assert!(hist.contains("assistant"));

        // 5. Test KV was persisted in SQLite
        let kv_val = db
            .get_extension_kv("community.agent-chat", "chat.history.agent_123")
            .await
            .expect("read kv")
            .expect("kv value exists");
        assert!(kv_val.contains("Hello from test"));
        assert!(kv_val.contains("Assistant reply from test"));

        // 6. Test isolated file storage was written
        let transcript_path = runner.storage_dir().join("transcript_agent_123.log");
        assert!(transcript_path.exists());
        let log_content = std::fs::read_to_string(transcript_path).unwrap();
        assert!(log_content.contains("User agent_123: Hello from test"));
        assert!(log_content.contains("Agent agent_123 (chat_send): Assistant reply from test"));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
