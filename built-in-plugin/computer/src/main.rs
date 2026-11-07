use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

const MAX_OUTPUT_BYTES: usize = 50 * 1024 * 1024; // 50MB cap

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessEntry {
    pid: u32,
    command: String,
    start_time: DateTime<Utc>,
    end_time: Option<DateTime<Utc>>,
    status: String, // "running", "completed", "failed", "detached_due_to_timeout", "killed"
    exit_code: Option<i32>,
    output: String,
}

struct AppState {
    terminals: Mutex<HashMap<u32, Arc<Mutex<ProcessEntry>>>>,
    stdout_lock: Mutex<()>,
}

impl AppState {
    fn new() -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            stdout_lock: Mutex::new(()),
        }
    }

    async fn send_json(&self, val: &Value) {
        let _guard = self.stdout_lock.lock().await;
        if let Ok(line) = serde_json::to_string(val) {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            let _ = out.flush();
        }
    }

    async fn send_notification(&self, method: &str, params: Value) {
        self.send_json(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await;
    }
}

fn parse_timeout(s: Option<&str>, default_ms: u64) -> u64 {
    let raw = match s {
        Some(val) => val.trim().to_lowercase(),
        None => return default_ms,
    };
    if raw.is_empty() {
        return default_ms;
    }

    let mut num_str = String::new();
    let mut unit_str = String::new();

    for c in raw.chars() {
        if c.is_ascii_digit() || c == '.' {
            num_str.push(c);
        } else if !c.is_whitespace() {
            unit_str.push(c);
        }
    }

    let num: f64 = num_str.parse().unwrap_or(0.0);
    if num <= 0.0 {
        return default_ms;
    }

    match unit_str.as_str() {
        "ms" => num as u64,
        "s" | "sec" | "secs" | "" => (num * 1000.0) as u64,
        "m" | "min" | "mins" => (num * 60.0 * 1000.0) as u64,
        "h" | "hr" | "hrs" => (num * 3600.0 * 1000.0) as u64,
        _ => (num * 1000.0) as u64,
    }
}

fn parse_signal(s: &str) -> i32 {
    match s.to_uppercase().as_str() {
        "SIGTERM" | "TERM" | "15" => libc::SIGTERM,
        "SIGKILL" | "KILL" | "9" => libc::SIGKILL,
        "SIGINT" | "INT" | "2" => libc::SIGINT,
        "SIGHUP" | "HUP" | "1" => libc::SIGHUP,
        "SIGQUIT" | "QUIT" | "3" => libc::SIGQUIT,
        _ => libc::SIGTERM,
    }
}

// -----------------------------------------------------------------------------
// MCP Protocol Handlers
// -----------------------------------------------------------------------------

fn get_tool_definitions() -> Value {
    json!([
        {
            "name": "terminal",
            "description": "Execute a bash command in the terminal environment. If execution exceeds the timeout, it automatically detaches into the background with a PID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bash": {
                        "type": "string",
                        "description": "The bash command to execute in shell environment"
                    },
                    "runInBackground": {
                        "type": "boolean",
                        "description": "Whether to run the command in background without waiting (default: false)"
                    },
                    "timeout": {
                        "type": "string",
                        "description": "Execution timeout duration, e.g. \"120s\", \"30s\", \"5m\" (default: \"120s\")"
                    }
                },
                "required": ["bash"]
            }
        },
        {
            "name": "list_terminal",
            "description": "List all managed terminal processes (running, detached, completed, or failed)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "activeOnly": {
                        "type": "boolean",
                        "description": "If true, only returns active/running or detached processes (default: false)"
                    }
                }
            }
        },
        {
            "name": "check_terminal",
            "description": "Check status and latest output for a specific terminal process ID (PID)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {
                        "type": "integer",
                        "description": "The process ID of the terminal to inspect"
                    }
                },
                "required": ["pid"]
            }
        },
        {
            "name": "kill_terminal",
            "description": "Send a termination signal (SIGTERM/SIGKILL) to a managed terminal process",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {
                        "type": "integer",
                        "description": "Process ID (PID) of the terminal process to kill"
                    },
                    "signal": {
                        "type": "string",
                        "description": "Signal to send (e.g. SIGTERM, SIGKILL). Default: SIGTERM"
                    }
                },
                "required": ["pid"]
            }
        },
        {
            "name": "list",
            "description": "List contents (files and directories) with metadata at the specified path",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of the directory to list (default: current directory .)"
                    }
                }
            }
        },
        {
            "name": "read",
            "description": "Read file content with optional 1-based line range (from / to)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute file path to read"
                    },
                    "from": {
                        "type": "integer",
                        "description": "Start line number (1-based, inclusive)"
                    },
                    "to": {
                        "type": "integer",
                        "description": "End line number (1-based, inclusive)"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "set",
            "description": "Write complete content to a file (creates parent directories if needed)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of the file to create or overwrite"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full text content to write into the file"
                    }
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "replace",
            "description": "Find and replace exact text string inside a file",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of the file to modify"
                    },
                    "search": {
                        "type": "string",
                        "description": "Exact text substring to search for"
                    },
                    "content": {
                        "type": "string",
                        "description": "Replacement text content"
                    }
                },
                "required": ["path", "search", "content"]
            }
        }
    ])
}

// -----------------------------------------------------------------------------
// Tool Implementations
// -----------------------------------------------------------------------------

async fn tool_terminal(
    state: &Arc<AppState>,
    args: &Value,
) -> Result<Value, String> {
    let command = args
        .get("bash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'bash' parameter".to_string())?
        .to_string();

    let run_in_background = args
        .get("runInBackground")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let timeout_str = args.get("timeout").and_then(|v| v.as_str());
    let timeout_ms = parse_timeout(timeout_str, 120_000);

    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c").arg(&command);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Create process group on Unix so child and its sub-processes can be signalled
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn process: {}", e))?;
    let pid = child.id().ok_or_else(|| "Failed to obtain child process PID".to_string())?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let entry = Arc::new(Mutex::new(ProcessEntry {
        pid,
        command: command.clone(),
        start_time: Utc::now(),
        end_time: None,
        status: "running".to_string(),
        exit_code: None,
        output: String::new(),
    }));

    {
        let mut map = state.terminals.lock().await;
        map.insert(pid, Arc::clone(&entry));
    }

    // Spawn reader tasks for stdout & stderr
    let entry_stdout = Arc::clone(&entry);
    if let Some(mut out_stream) = stdout {
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = out_stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut e = entry_stdout.lock().await;
                e.output.push_str(&chunk);
                if e.output.len() > MAX_OUTPUT_BYTES {
                    let trim_pos = e.output.len() - MAX_OUTPUT_BYTES;
                    e.output.drain(..trim_pos);
                }
            }
        });
    }

    let entry_stderr = Arc::clone(&entry);
    if let Some(mut err_stream) = stderr {
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err_stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut e = entry_stderr.lock().await;
                e.output.push_str(&chunk);
                if e.output.len() > MAX_OUTPUT_BYTES {
                    let trim_pos = e.output.len() - MAX_OUTPUT_BYTES;
                    e.output.drain(..trim_pos);
                }
            }
        });
    }

    // Monitor child exit in background
    let entry_monitor = Arc::clone(&entry);
    let state_monitor = Arc::clone(state);
    let command_clone = command.clone();
    tokio::spawn(async move {
        let status_res = child.wait().await;
        let now = Utc::now();
        let (exit_code, status_str, duration_secs, output_snapshot) = {
            let mut e = entry_monitor.lock().await;
            e.end_time = Some(now);
            let code = status_res.ok().and_then(|s| s.code()).unwrap_or(-1);
            e.exit_code = Some(code);
            e.status = if code == 0 { "completed".to_string() } else { "failed".to_string() };
            let dur = (now - e.start_time).num_seconds().max(0);
            (code, e.status.clone(), dur, e.output.clone())
        };

        let notification_payload = json!({
            "uri": "terminal://completed",
            "pid": pid,
            "command": command_clone,
            "status": status_str,
            "exit_code": exit_code,
            "duration_seconds": duration_secs,
            "output": output_snapshot,
            "message": format!("Terminal PID {} ('{}') {} with exit code {} in {}s.", pid, command_clone, status_str, exit_code, duration_secs)
        });

        // 1. Standard MCP Resource update
        state_monitor
            .send_notification("notifications/resources/updated", json!({ "uri": "terminal://completed" }))
            .await;

        // 2. Direct message push notification for agent wakeup
        state_monitor
            .send_notification("notifications/message", notification_payload)
            .await;
    });

    if run_in_background {
        return Ok(json!({
            "status": "running_in_background",
            "pid": pid,
            "command": command,
            "message": format!("Process started in background with PID {}. Use check_terminal(pid: {}) to inspect output.", pid, pid)
        }));
    }

    // Synchronous wait with timeout
    let timeout_duration = Duration::from_millis(timeout_ms);
    let start_wait = tokio::time::Instant::now();

    loop {
        {
            let e = entry.lock().await;
            if e.end_time.is_some() {
                return Ok(json!({
                    "status": e.status,
                    "exit_code": e.exit_code.unwrap_or(0),
                    "output": if e.output.is_empty() { "(no output produced)" } else { &e.output }
                }));
            }
        }

        if start_wait.elapsed() >= timeout_duration {
            let mut e = entry.lock().await;
            e.status = "detached_due_to_timeout".to_string();
            let out_so_far = if e.output.is_empty() { "(no output produced yet)".to_string() } else { e.output.clone() };
            return Ok(json!({
                "status": "detached_due_to_timeout",
                "pid": pid,
                "command": command,
                "timeout": timeout_str.unwrap_or("120s"),
                "message": format!("Command exceeded timeout of {}. Process was automatically detached into the background with PID {}. Use check_terminal(pid: {}) to inspect output.", timeout_str.unwrap_or("120s"), pid, pid),
                "output_so_far": out_so_far
            }));
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn tool_list_terminal(
    state: &Arc<AppState>,
    args: &Value,
) -> Result<Value, String> {
    let active_only = args.get("activeOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let now = Utc::now();

    let map = state.terminals.lock().await;
    let mut list = Vec::new();

    for (pid, entry_arc) in map.iter() {
        let e = entry_arc.lock().await;
        let is_active = e.status == "running" || e.status == "detached_due_to_timeout";
        if active_only && !is_active {
            continue;
        }

        let dur = match e.end_time {
            Some(end) => (end - e.start_time).num_seconds().max(0),
            None => (now - e.start_time).num_seconds().max(0),
        };

        list.push(json!({
            "pid": pid,
            "command": e.command,
            "status": e.status,
            "exit_code": e.exit_code,
            "duration_seconds": dur,
            "output_length_chars": e.output.len(),
            "start_time": e.start_time.to_rfc3339(),
            "end_time": e.end_time.map(|t| t.to_rfc3339())
        }));
    }

    Ok(json!({
        "count": list.len(),
        "terminals": list
    }))
}

async fn tool_check_terminal(
    state: &Arc<AppState>,
    args: &Value,
) -> Result<Value, String> {
    let pid = args
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing or invalid 'pid' parameter".to_string())? as u32;

    let map = state.terminals.lock().await;
    let entry_arc = map
        .get(&pid)
        .ok_or_else(|| format!("No managed terminal process found with PID {}. Use list_terminal() to view available PIDs.", pid))?;

    let e = entry_arc.lock().await;
    let now = Utc::now();
    let dur = match e.end_time {
        Some(end) => (end - e.start_time).num_seconds().max(0),
        None => (now - e.start_time).num_seconds().max(0),
    };

    Ok(json!({
        "pid": e.pid,
        "command": e.command,
        "status": e.status,
        "exit_code": e.exit_code,
        "duration_seconds": dur,
        "output": if e.output.is_empty() { "(no output produced yet)" } else { &e.output },
        "total_output_chars": e.output.len(),
        "start_time": e.start_time.to_rfc3339(),
        "end_time": e.end_time.map(|t| t.to_rfc3339())
    }))
}

async fn tool_kill_terminal(
    state: &Arc<AppState>,
    args: &Value,
) -> Result<Value, String> {
    let pid = args
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing or invalid 'pid' parameter".to_string())? as u32;

    let signal_str = args.get("signal").and_then(|v| v.as_str()).unwrap_or("SIGTERM");
    let sig = parse_signal(signal_str);

    let map = state.terminals.lock().await;
    let entry_arc = map
        .get(&pid)
        .ok_or_else(|| format!("No managed terminal process found with PID {}", pid))?;

    let mut e = entry_arc.lock().await;

    // Send signal to process group first (-pid), fallback to single pid
    let ret = unsafe { libc::kill(-(pid as i32), sig) };
    if ret != 0 {
        let ret2 = unsafe { libc::kill(pid as i32, sig) };
        if ret2 != 0 {
            let err = io::Error::last_os_error();
            return Err(format!("Failed to kill process {}: {}", pid, err));
        }
    }

    e.status = "killed".to_string();
    e.end_time = Some(Utc::now());

    Ok(json!({
        "pid": pid,
        "status": "killed",
        "signal": signal_str,
        "message": format!("Signal {} sent to process {}", signal_str, pid)
    }))
}

fn tool_list(args: &Value) -> Result<Value, String> {
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let target = Path::new(path_str);
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("Directory path does not exist: {} ({})", path_str, e))?;

    let read_dir = fs::read_dir(&canonical).map_err(|e| format!("Failed to read directory {}: {}", canonical.display(), e))?;
    let mut items = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().ok();
        let file_type = match meta.as_ref() {
            Some(m) if m.is_dir() => "directory",
            Some(m) if m.is_file() => "file",
            _ => "other",
        };
        let size_bytes = meta.as_ref().and_then(|m| if m.is_file() { Some(m.len()) } else { None });
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| DateTime::<Utc>::from(t).to_rfc3339());

        items.push(json!({
            "name": name,
            "type": file_type,
            "size_bytes": size_bytes,
            "modified": modified
        }));
    }

    Ok(json!({
        "path": canonical.display().to_string(),
        "count": items.len(),
        "items": items
    }))
}

fn tool_read(args: &Value) -> Result<Value, String> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

    let target = Path::new(path_str);
    if !target.exists() {
        return Err(format!("File does not exist: {}", path_str));
    }

    let raw = fs::read_to_string(target).map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;
    let lines: Vec<&str> = raw.lines().collect();
    let total_lines = lines.len();

    let from = args.get("from").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(1).max(1);
    let to = args.get("to").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(total_lines).min(total_lines);

    if from > to && total_lines > 0 {
        return Err(format!("'from' line ({}) cannot be greater than 'to' line ({})", from, to));
    }

    let selected = if total_lines == 0 {
        Vec::new()
    } else {
        lines[(from - 1)..to].to_vec()
    };

    let numbered: Vec<String> = selected
        .iter()
        .enumerate()
        .map(|(idx, line)| format!("{} | {}", from + idx, line))
        .collect();

    Ok(json!({
        "path": path_str,
        "from": from,
        "to": to,
        "total_lines": total_lines,
        "content": numbered.join("\n"),
        "raw_content": selected.join("\n")
    }))
}

fn tool_set(args: &Value) -> Result<Value, String> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'content' parameter".to_string())?;

    let path = PathBuf::from(path_str);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent directories for {}: {}", path_str, e))?;
        }
    }

    fs::write(&path, content).map_err(|e| format!("Failed to write file {}: {}", path_str, e))?;

    Ok(json!({
        "success": true,
        "path": path_str,
        "bytes_written": content.len(),
        "message": format!("Successfully wrote file: {}", path_str)
    }))
}

fn tool_replace(args: &Value) -> Result<Value, String> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

    let search = args
        .get("search")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required 'search' target string".to_string())?;

    let replacement = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required replacement 'content' string".to_string())?;

    let target = Path::new(path_str);
    if !target.exists() {
        return Err(format!("File does not exist: {}", path_str));
    }

    let original = fs::read_to_string(target).map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;

    if !original.contains(search) {
        return Err(format!("Search string target not found in {}", path_str));
    }

    let occurrences = original.matches(search).count();
    let replaced = original.replace(search, replacement);

    fs::write(target, replaced).map_err(|e| format!("Failed to write replaced file {}: {}", path_str, e))?;

    Ok(json!({
        "success": true,
        "path": path_str,
        "occurrences_replaced": occurrences,
        "message": format!("Successfully replaced {} occurrences in {}", occurrences, path_str)
    }))
}

// -----------------------------------------------------------------------------
// Request Dispatcher
// -----------------------------------------------------------------------------

async fn handle_request(state: &Arc<AppState>, req: Value) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (no 'id')
    if id.is_none() {
        if method == "notifications/initialized" {
            eprintln!("[computer-mcp] Client initialized notification received");
        }
        return;
    }

    let req_id = id.unwrap();

    match method {
        "initialize" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "computer-mcp",
                        "version": "2.0.0"
                    }
                }
            });
            state.send_json(&res).await;
        }

        "ping" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {}
            });
            state.send_json(&res).await;
        }

        "tools/list" => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "tools": get_tool_definitions()
                }
            });
            state.send_json(&res).await;
        }

        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let result: Result<Value, String> = match tool_name {
                "terminal" | "execute" => tool_terminal(state, &arguments).await,
                "list_terminal" => tool_list_terminal(state, &arguments).await,
                "check_terminal" => tool_check_terminal(state, &arguments).await,
                "kill_terminal" => tool_kill_terminal(state, &arguments).await,
                "list" => tool_list(&arguments),
                "read" => tool_read(&arguments),
                "set" => tool_set(&arguments),
                "replace" => tool_replace(&arguments),
                other => Err(format!("Unknown tool: {}", other)),
            };

            let response_body = match result {
                Ok(val) => {
                    let text_content = if val.is_string() {
                        val.as_str().unwrap().to_string()
                    } else {
                        val.to_string()
                    };
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": text_content
                                }
                            ],
                            "isError": false
                        }
                    })
                }
                Err(err_msg) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": err_msg
                                }
                            ],
                            "isError": true
                        }
                    })
                }
            };

            state.send_json(&response_body).await;
        }

        other => {
            let res = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {}", other)
                }
            });
            state.send_json(&res).await;
        }
    }
}

// -----------------------------------------------------------------------------
// Main Loop
// -----------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[computer-mcp] Starting Computer MCP binary stdio server v2.0.0");

    let state = Arc::new(AppState::new());
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut tasks = tokio::task::JoinSet::new();

    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(trimmed) {
            Ok(req) => {
                let state_clone = Arc::clone(&state);
                tasks.spawn(async move {
                    handle_request(&state_clone, req).await;
                });
            }
            Err(e) => {
                eprintln!("[computer-mcp] Invalid JSON received: {} (error: {})", trimmed, e);
                let err_res = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                state.send_json(&err_res).await;
            }
        }
    }

    // Drain in-flight tasks before closing
    while tasks.join_next().await.is_some() {}

    eprintln!("[computer-mcp] Stdin EOF reached, shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_timeout() {
        assert_eq!(parse_timeout(Some("120s"), 1000), 120_000);
        assert_eq!(parse_timeout(Some("5m"), 1000), 300_000);
        assert_eq!(parse_timeout(Some("500ms"), 1000), 500);
        assert_eq!(parse_timeout(Some("1h"), 1000), 3_600_000);
        assert_eq!(parse_timeout(None, 60_000), 60_000);
        assert_eq!(parse_timeout(Some(""), 60_000), 60_000);
    }

    #[test]
    fn test_parse_signal() {
        assert_eq!(parse_signal("SIGTERM"), libc::SIGTERM);
        assert_eq!(parse_signal("SIGKILL"), libc::SIGKILL);
        assert_eq!(parse_signal("9"), libc::SIGKILL);
        assert_eq!(parse_signal("SIGINT"), libc::SIGINT);
    }

    #[test]
    fn test_tool_definitions() {
        let defs = get_tool_definitions();
        let arr = defs.as_array().expect("array of tools");
        assert_eq!(arr.len(), 8);
        let names: Vec<&str> = arr.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"terminal"));
        assert!(names.contains(&"list_terminal"));
        assert!(names.contains(&"check_terminal"));
        assert!(names.contains(&"kill_terminal"));
        assert!(names.contains(&"list"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"set"));
        assert!(names.contains(&"replace"));
    }

    #[test]
    fn test_fs_tools_set_read_replace() {
        let tmp_dir = std::env::temp_dir().join(format!("test_mcp_{}", std::process::id()));
        let tmp_file = tmp_dir.join("test.txt");
        let path_str = tmp_file.to_str().unwrap();

        // 1. set
        let set_res = tool_set(&json!({
            "path": path_str,
            "content": "line 1\nline 2\nline 3"
        })).expect("tool_set succeeds");
        assert_eq!(set_res["success"], true);

        // 2. read
        let read_res = tool_read(&json!({
            "path": path_str,
            "from": 2,
            "to": 3
        })).expect("tool_read succeeds");
        assert_eq!(read_res["total_lines"], 3);
        assert_eq!(read_res["from"], 2);
        assert_eq!(read_res["to"], 3);
        assert!(read_res["content"].as_str().unwrap().contains("2 | line 2"));

        // 3. replace
        let replace_res = tool_replace(&json!({
            "path": path_str,
            "search": "line 2",
            "content": "modified 2"
        })).expect("tool_replace succeeds");
        assert_eq!(replace_res["occurrences_replaced"], 1);

        // 4. read after replace
        let read_after = tool_read(&json!({
            "path": path_str
        })).expect("tool_read succeeds");
        assert!(read_after["raw_content"].as_str().unwrap().contains("modified 2"));

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
