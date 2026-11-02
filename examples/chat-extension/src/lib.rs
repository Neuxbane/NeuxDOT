use serde::{Deserialize, Serialize};

// --- Host Imports (Sandboxed Storage & Process Execution) ---
#[link(wasm_import_module = "env")]
extern "C" {
    fn host_log(msg_ptr: *const u8, msg_len: usize);
    fn host_kv_get(key_ptr: *const u8, key_len: usize, out_ptr: *mut u8, max_out: usize) -> i32;
    fn host_kv_set(key_ptr: *const u8, key_len: usize, val_ptr: *const u8, val_len: usize) -> i32;
    fn host_kv_delete(key_ptr: *const u8, key_len: usize) -> i32;
    fn host_fs_write(path_ptr: *const u8, path_len: usize, data_ptr: *const u8, data_len: usize) -> i32;
    fn host_agent_run_background(
        agent_id_ptr: *const u8,
        agent_id_len: usize,
        input_ptr: *const u8,
        input_len: usize,
    ) -> i32;
    fn host_get_time_str(out_ptr: *mut u8, max_out: usize) -> i32;
}

fn log(msg: &str) {
    unsafe {
        host_log(msg.as_ptr(), msg.len());
    }
}

fn get_current_time() -> String {
    let mut buf = vec![0u8; 64];
    let len = unsafe { host_get_time_str(buf.as_mut_ptr(), buf.len()) };
    if len > 0 {
        buf.truncate(len as usize);
        String::from_utf8(buf).unwrap_or_else(|_| String::from("12:00 PM"))
    } else {
        String::from("12:00 PM")
    }
}

fn kv_get(key: &str) -> Option<String> {
    let mut buf = vec![0u8; 65536];
    let len = unsafe { host_kv_get(key.as_ptr(), key.len(), buf.as_mut_ptr(), buf.len()) };
    if len >= 0 {
        buf.truncate(len as usize);
        String::from_utf8(buf).ok()
    } else {
        None
    }
}

fn kv_set(key: &str, val: &str) -> bool {
    let res = unsafe { host_kv_set(key.as_ptr(), key.len(), val.as_ptr(), val.len()) };
    res == 0
}

fn kv_delete(key: &str) -> bool {
    let res = unsafe { host_kv_delete(key.as_ptr(), key.len()) };
    res == 0
}

fn fs_write(path: &str, data: &[u8]) -> bool {
    let res = unsafe { host_fs_write(path.as_ptr(), path.len(), data.as_ptr(), data.len()) };
    res == 0
}

// Memory allocator for host to pass data into WASM
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr
}

fn pack_string_response(s: String) -> i64 {
    let bytes = s.into_bytes();
    let ptr = bytes.as_ptr() as usize;
    let len = bytes.len();
    core::mem::forget(bytes);
    ((ptr as i64) << 32) | (len as i64)
}

#[derive(Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" or "assistant"
    pub text: String,
    pub timestamp: String,
}

// --- Plugin-owned storage keys ---------------------------------------
// Every key this plugin reads or writes lives under the `chat.` prefix so it
// matches the manifest's `storage.keys: ["chat."]` allow-list
// (HOST_BRIDGE.md §1/§2). FS paths are *not* prefixed: they are files, not
// KV keys, but they are named consistently for the transcript log.
const HISTORY_KEY_PREFIX: &str = "chat.history.";
const ACTIVE_AGENT_KEY: &str = "chat.active_agent_id";
const TRANSCRIPT_DIR: &str = "chat.transcripts";

fn transcript_path(agent_id: &str) -> String {
    format!("{}/transcript_{}.log", TRANSCRIPT_DIR, agent_id)
}

#[derive(Deserialize)]
struct McpCallToolPayload {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct McpReadResourcePayload {
    uri: String,
}

/// 1. Return Plugin Manifest with standard MCP-Apps (`ui://`) resources and capabilities
#[no_mangle]
pub extern "C" fn get_manifest() -> i64 {
    let manifest = r#"{
  "id": "community.agent-chat",
  "name": "Agent Chat Console",
  "version": "2.0.0",
  "description": "Pure MCP-App interactive chat console for agents. Serves UI over ui://chat-console and communicates exclusively via standard MCP JSON-RPC.",
  "permissions": {
    "ui": {
      "enabled": true,
      "resources": ["ui://chat-console"]
    },
    "storage": {
      "get": true,
      "set": true,
      "delete": true,
      "keys": ["chat."]
    },
    "agent": {
      "trigger": true,
      "tools": true
    },
    "identity": {
      "agent.name": true,
      "agent.profile_picture": true,
      "user.name": true,
      "user.profile_picture": true
    },
    "process": false,
    "network": false
  },
  "mcp": {
    "server_name": "Agent Chat Console",
    "alias": "agent_chat_console",
    "tools": [
      {
        "name": "chat_send",
        "description": "Send an assistant reply message into the interactive chat console",
        "inputSchema": {
          "type": "object",
          "properties": {
            "agent_id": { "type": "string", "description": "Optional target agent ID" },
            "message": { "type": "string", "description": "The message text to send into the chat console" }
          },
          "required": ["message"]
        },
        "_meta": {
          "ui": {
            "resourceUri": "ui://chat-console"
          }
        }
      },
      {
        "name": "chat_user_message",
        "description": "Send a user message from the MCP-App UI and trigger background agent execution",
        "inputSchema": {
          "type": "object",
          "properties": {
            "agent_id": { "type": "string", "description": "Target agent ID" },
            "message": { "type": "string", "description": "User prompt message" }
          },
          "required": ["agent_id", "message"]
        },
        "_meta": {
          "ui": {
            "resourceUri": "ui://chat-console",
            "visibility": ["app"]
          }
        }
      },
      {
        "name": "chat_history",
        "description": "Retrieve conversation history for the specified agent",
        "inputSchema": {
          "type": "object",
          "properties": {
            "agent_id": { "type": "string", "description": "Target agent ID" }
          },
          "required": ["agent_id"]
        },
        "_meta": {
          "ui": {
            "resourceUri": "ui://chat-console",
            "visibility": ["app"]
          }
        }
      },
      {
        "name": "chat_clear",
        "description": "Clear conversation history for the specified agent",
        "inputSchema": {
          "type": "object",
          "properties": {
            "agent_id": { "type": "string", "description": "Target agent ID" }
          },
          "required": ["agent_id"]
        },
        "_meta": {
          "ui": {
            "resourceUri": "ui://chat-console",
            "visibility": ["app"]
          }
        }
      }
    ],
    "resources": [
      {
        "uri": "ui://chat-console",
        "name": "Interactive Chat",
        "description": "Sandboxed MCP-App interactive chat interface",
        "mimeType": "text/html;profile=mcp-app"
      },
      {
        "uri": "extension://chat/messages",
        "name": "Live Chat Messages Stream",
        "description": "Real-time updates of messages from user chat console (Push MCP Subscriptions)",
        "mimeType": "application/json"
      }
    ]
  }
}"#;
    pack_string_response(String::from(manifest))
}

const CHAT_APP_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Interactive Chat — MCP App</title>
<style>
  :root {
    --bg: #ffffff;
    --surface: #fafafa;
    --border: #e4e4e7;
    --text: #09090b;
    --muted: #71717a;
    --primary: #18181b;
    --primary-fg: #ffffff;
    --user-bubble: #18181b;
    --user-fg: #ffffff;
    --bot-bubble: #f4f4f5;
    --bot-fg: #09090b;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --bg: #09090b;
      --surface: #121215;
      --border: #27272a;
      --text: #f4f4f5;
      --muted: #a1a1aa;
      --primary: #f4f4f5;
      --primary-fg: #09090b;
      --user-bubble: #27272a;
      --user-fg: #f4f4f5;
      --bot-bubble: #18181b;
      --bot-fg: #f4f4f5;
    }
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: var(--bg);
    color: var(--text);
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .btn-clear {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--muted);
    padding: 4px 10px;
    border-radius: 8px;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .btn-clear:hover { color: #ef4444; border-color: #ef4444; }
  #messages {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .empty {
    margin: auto;
    text-align: center;
    color: var(--muted);
    font-size: 12px;
    max-width: 320px;
    line-height: 1.5;
    display: flex;
    flex-direction: column;
    align-items: center;
  }
  .empty-avatar-wrap {
    margin-bottom: 12px;
  }
  .empty-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
    margin-bottom: 4px;
  }
  .empty-desc {
    color: var(--muted);
    font-size: 12px;
  }
  .msg-row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    max-width: 84%;
  }
  .msg-row.user {
    align-self: flex-end;
    flex-direction: row-reverse;
  }
  .msg-row.assistant {
    align-self: flex-start;
  }
  .msg-content {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .msg-row.user .msg-content {
    align-items: flex-end;
  }
  .msg-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 4px;
    font-size: 11px;
  }
  .msg-row.user .msg-header {
    flex-direction: row-reverse;
  }
  .msg-name {
    font-weight: 600;
    color: var(--text);
  }
  .msg-time {
    color: var(--muted);
    font-size: 10px;
  }
  .msg-bubble {
    padding: 10px 14px;
    border-radius: 14px;
    font-size: 13px;
    line-height: 1.45;
    word-break: break-word;
    white-space: pre-wrap;
  }
  .msg-bubble.user {
    background: var(--user-bubble);
    color: var(--user-fg);
    border-top-right-radius: 3px;
  }
  .msg-bubble.assistant {
    background: var(--bot-bubble);
    color: var(--bot-fg);
    border: 1px solid var(--border);
    border-top-left-radius: 3px;
  }
  footer {
    padding: 12px 16px;
    border-top: 1px solid var(--border);
    background: var(--surface);
    display: flex;
    gap: 8px;
  }
  input {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 12px;
    padding: 9px 14px;
    font-size: 13px;
    outline: none;
    transition: border-color 0.15s ease;
  }
  input:focus { border-color: var(--primary); }
  button.send {
    background: var(--primary);
    color: var(--primary-fg);
    border: none;
    border-radius: 12px;
    padding: 0 18px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }
  button.send:hover { opacity: 0.9; }
  button.send:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
</head>
<body>
  <header>
    <div style="display:flex;align-items:center;gap:10px;">
      <div id="header-avatar"></div>
      <div>
        <div id="header-name" style="font-size:12px;font-weight:600;color:var(--text);line-height:1.2;">Chat</div>
        <div id="header-sub" style="font-size:10px;color:var(--muted);line-height:1.2;">Connected</div>
      </div>
    </div>
    <div style="display:flex;align-items:center;gap:6px;">
      <button class="btn-clear" id="refresh-btn" title="Refresh context and history">Refresh</button>
      <button class="btn-clear" id="clear-btn">Clear History</button>
    </div>
  </header>

  <div id="messages">
    <div class="empty">
      <div class="empty-title">Loading session...</div>
    </div>
  </div>

  <footer>
    <input id="chat-input" type="text" placeholder="Message agent..." autocomplete="off" />
    <button class="send" id="send-btn">Send</button>
  </footer>

  <script>
    // --- Identity state -------------------------------------------------
    // Mirrors HOST_BRIDGE.md §3: every field below is OPTIONAL. The host
    // omits anything that was not granted, so `null` here always means
    // "not known", and the UI renders its own neutral placeholders.
    const DEFAULT_AGENT_COLOR = '#3b82f6';
    const DEFAULT_USER_COLOR = '#10b981';
    const NEUTRAL_AGENT_LABEL = 'Chat';       // never invent an agent name
    const NEUTRAL_USER_LABEL = 'User';

    let agentId = '';
    let agentName = null;          // string | null
    let agentAvatarUrl = null;     // string | null
    let agentAvatarColor = DEFAULT_AGENT_COLOR;
    let agentAvatarShape = 'circle';
    let userName = null;           // string | null
    let userAvatarUrl = null;      // string | null
    let userAvatarColor = DEFAULT_USER_COLOR;
    let userAvatarShape = 'circle';
    let capabilities = [];         // granted capability strings from bootstrap (diagnostics only)
    let theme = null;

    let rpcId = 1;
    const pending = new Map();

    // Returns a plain string or null — never the literal `undefined`.
    function asText(value) {
      return typeof value === 'string' && value.trim() !== '' ? value.trim() : null;
    }

    // Returns a string or null.
    function asColor(value) {
      return typeof value === 'string' && /^#[0-9a-fA-F]{3,8}$/.test(value.trim())
        ? value.trim()
        : null;
    }

    function asShape(value) {
      return value === 'circle' || value === 'rounded' || value === 'square' ? value : 'circle';
    }

    // Only https:, http: and data:image/ may be handed to an <img>. Anything
    // else (file:, blob:, javascript:, relative paths, ...) would be refused
    // by the opaque-origin sandbox and surface as a console error.
    const SAFE_IMG_SRC = /^(https?:|data:image\/)/i;
    function isSafeImageUrl(url) {
      return typeof url === 'string' && SAFE_IMG_SRC.test(url.trim());
    }

    function initialOf(name) {
      const n = asText(name);
      if (!n) return '?';
      const ch = Array.from(n)[0];
      return ch ? ch.toUpperCase() : '?';
    }

    function borderRadiusFor(shape) {
      if (shape === 'square') return '8px';
      if (shape === 'rounded') return '10px';
      return '50%';
    }

    // Builds an avatar as real DOM nodes (createElement + textContent), so a
    // hostile display name or colour can never inject markup. Returns an
    // <img> only for https:/http:/data:image/ URLs; always with an onerror
    // fallback to the coloured initial.
    function makeAvatar(name, url, color, shape, size) {
      const px = Math.max(16, Number(size) || 28);
      const radius = borderRadiusFor(shape);
      const baseStyle = 'width:' + px + 'px;height:' + px + 'px;border-radius:' + radius +
        ';object-fit:cover;flex-shrink:0;overflow:hidden;';

      const fallback = () => {
        const div = document.createElement('div');
        div.className = 'avatar-fallback';
        div.setAttribute('aria-hidden', 'true');
        div.style.cssText = baseStyle + 'background:' + (asColor(color) || DEFAULT_AGENT_COLOR) +
          ';display:flex;align-items:center;justify-content:center;color:#fff;font-weight:600;font-size:' +
          Math.round(px * 0.42) + 'px;user-select:none;';
        div.textContent = initialOf(name);
        return div;
      };

      if (!isSafeImageUrl(url)) return fallback();

      const img = document.createElement('img');
      img.className = 'avatar-img';
      img.style.cssText = baseStyle;
      img.alt = asText(name) || '';
      img.referrerPolicy = 'no-referrer';
      img.decoding = 'async';
      // Simple, quote-free fallback: swap the node itself for the initial.
      img.addEventListener('error', () => {
        if (img.parentNode) img.parentNode.replaceChild(fallback(), img);
      }, { once: true });
      img.src = url.trim();
      return img;
    }

    function replaceChildren(el, node) {
      if (!el) return;
      while (el.firstChild) el.removeChild(el.firstChild);
      el.appendChild(node);
    }

    // Reads the nested bootstrap from HOST_BRIDGE.md §3:
    //   { agent: { id, name?, avatar: { url?, color?, shape? } }, user: {...}, ... }
    // Absent fields stay null; nothing here throws on a partial payload.
    function updateContext(ctx) {
      if (!ctx || typeof ctx !== 'object') return;

      const agent = (ctx.agent && typeof ctx.agent === 'object') ? ctx.agent : null;
      const user = (ctx.user && typeof ctx.user === 'object') ? ctx.user : null;
      const agentAvatar = (agent && agent.avatar && typeof agent.avatar === 'object') ? agent.avatar : null;
      const userAvatar = (user && user.avatar && typeof user.avatar === 'object') ? user.avatar : null;

      agentId = asText(agent && agent.id) || agentId || '';
      agentName = asText(agent && agent.name);
      agentAvatarUrl = asText(agentAvatar && agentAvatar.url);
      agentAvatarColor = asColor(agentAvatar && agentAvatar.color) || DEFAULT_AGENT_COLOR;
      agentAvatarShape = asShape(agentAvatar && agentAvatar.shape);

      userName = asText(user && user.name);
      userAvatarUrl = asText(userAvatar && userAvatar.url);
      userAvatarColor = asColor(userAvatar && userAvatar.color) || DEFAULT_USER_COLOR;
      userAvatarShape = asShape(userAvatar && userAvatar.shape);

      capabilities = Array.isArray(ctx.capabilities)
        ? ctx.capabilities.filter((c) => typeof c === 'string')
        : [];
      const ctxInfo = (ctx.context && typeof ctx.context === 'object') ? ctx.context : null;
      theme = ctxInfo && ctxInfo.theme === 'light' ? 'light' : (ctxInfo && ctxInfo.theme === 'dark' ? 'dark' : null);
      if (theme) document.documentElement.setAttribute('data-theme', theme);

      renderHeader();
    }

    const displayAgentName = () => agentName || NEUTRAL_AGENT_LABEL;
    const displayUserName = () => userName || NEUTRAL_USER_LABEL;

    function renderHeader() {
      replaceChildren(
        document.getElementById('header-avatar'),
        makeAvatar(agentName, agentAvatarUrl, agentAvatarColor, agentAvatarShape, 28)
      );
      const nameSlot = document.getElementById('header-name');
      if (nameSlot) nameSlot.textContent = displayAgentName();
      const subSlot = document.getElementById('header-sub');
      if (subSlot) subSlot.textContent = userName ? 'Chatting as ' + userName : 'Connected';

      const input = document.getElementById('chat-input');
      if (input) input.placeholder = 'Message ' + displayAgentName() + '...';
    }

    // All outbound traffic goes through one helper: JSON-RPC 2.0 request or
    // notification (HOST_BRIDGE.md §4). The message body is plain data, so it
    // is structured-cloneable; the try/catch turns a clone mistake into a
    // console error instead of an uncaught DOMException (§8).
    function rpc(method, params, opts) {
      const isNotification = !!(opts && opts.notification);
      return new Promise((resolve, reject) => {
        let id = null;
        if (!isNotification) {
          id = rpcId++;
          pending.set(id, { resolve, reject });
          setTimeout(() => {
            if (pending.has(id)) {
              pending.delete(id);
              reject({ code: -32000, message: 'timeout: ' + method });
            }
          }, 15000);
        }
        const message = { jsonrpc: '2.0', method, params: params || {} };
        if (id !== null) message.id = id;
        try {
          window.parent.postMessage(message, '*');
        } catch (e) {
          if (id !== null) pending.delete(id);
          console.error('postMessage failed for ' + method, e);
          reject({ code: -32000, message: 'postMessage failed' });
          return;
        }
        if (isNotification) resolve(undefined);
      });
    }

    function mcpRequest(method, params) {
      return rpc(method, params);
    }

    // `host/call` (contract §4) — only ever used with capabilities that the
    // manifest declares and the bootstrap advertises.
    function hostCall(capability, args) {
      return rpc('host/call', { capability, args: args || {} });
    }

    // Re-fetch the bootstrap (contract §1: granted by ui.enabled).
    async function refreshContext() {
      try {
        const ctx = await hostCall('ui.context.refresh', {});
        const value = ctx && ctx.value;
        if (value && typeof value === 'object') updateContext(value);
        return value || null;
      } catch (e) {
        console.warn('ui/context.refresh unavailable:', e && e.message ? e.message : e);
        return null;
      }
    }

    window.addEventListener('message', (event) => {
      const data = event.data;
      if (!data || data.jsonrpc !== '2.0') return;
      if (data.id !== undefined && data.id !== null && pending.has(data.id)) {
        const { resolve, reject } = pending.get(data.id);
        pending.delete(data.id);
        if (data.error) reject(data.error);
        else resolve(data.result);
      } else if (data.method === 'ui/context') {
        updateContext(data.params);
        loadHistory();
      } else if (data.method === 'host/storage.changed') {
        // Advisory only; ignore unknown shapes.
        loadHistory();
      }
    });

    function parseToolJson(res) {
      try {
        const text = (res && res.content && res.content[0] && res.content[0].text) || '[]';
        const parsed = JSON.parse(text);
        return Array.isArray(parsed) ? parsed : [];
      } catch {
        return [];
      }
    }

    // 100% DOM-built: no user-controlled string is ever parsed as HTML.
    function renderEmpty() {
      const container = document.getElementById('messages');
      if (!container) return;

      const wrap = document.createElement('div');
      wrap.className = 'empty';

      const avatarWrap = document.createElement('div');
      avatarWrap.className = 'empty-avatar-wrap';
      avatarWrap.appendChild(
        makeAvatar(agentName, agentAvatarUrl, agentAvatarColor, agentAvatarShape, 52)
      );
      wrap.appendChild(avatarWrap);

      const title = document.createElement('div');
      title.className = 'empty-title';
      title.textContent = 'Chat with ' + displayAgentName();
      wrap.appendChild(title);

      const desc = document.createElement('div');
      desc.className = 'empty-desc';
      desc.textContent = agentName
        ? 'Send a message to start chatting with ' + agentName + ' over sandboxed MCP JSON-RPC.'
        : 'Send a message to start chatting over sandboxed MCP JSON-RPC.';
      wrap.appendChild(desc);

      replaceChildren(container, wrap);
    }

    // 100% DOM-built: message text, names and timestamps are set through
    // textContent, so `<script>` or `<img onerror=...>` in a display name or
    // message is rendered literally instead of being executed.
    function renderMessages(list) {
      const container = document.getElementById('messages');
      if (!container) return;
      if (!Array.isArray(list) || list.length === 0) {
        renderEmpty();
        return;
      }
      while (container.firstChild) container.removeChild(container.firstChild);
      for (const m of list) {
        const isUser = m && m.role === 'user';
        const name = isUser ? displayUserName() : displayAgentName();

        const row = document.createElement('div');
        row.className = 'msg-row ' + (isUser ? 'user' : 'assistant');

        row.appendChild(
          isUser
            ? makeAvatar(userName, userAvatarUrl, userAvatarColor, userAvatarShape, 28)
            : makeAvatar(agentName, agentAvatarUrl, agentAvatarColor, agentAvatarShape, 28)
        );

        const content = document.createElement('div');
        content.className = 'msg-content';

        const header = document.createElement('div');
        header.className = 'msg-header';

        const nameEl = document.createElement('span');
        nameEl.className = 'msg-name';
        nameEl.textContent = name;
        header.appendChild(nameEl);

        const timeEl = document.createElement('span');
        timeEl.className = 'msg-time';
        timeEl.textContent = asText(m && m.timestamp) || '';
        header.appendChild(timeEl);

        content.appendChild(header);

        const bubble = document.createElement('div');
        bubble.className = 'msg-bubble ' + (isUser ? 'user' : 'assistant');
        bubble.textContent = (m && typeof m.text === 'string') ? m.text : '';
        content.appendChild(bubble);

        row.appendChild(content);
        container.appendChild(row);
      }
      container.scrollTop = container.scrollHeight;
    }

    async function loadHistory() {
      if (!agentId) return;
      try {
        const res = await mcpRequest('tools/call', {
          name: 'chat_history',
          arguments: { agent_id: agentId }
        });
        renderMessages(parseToolJson(res));
      } catch (e) {
        console.error('Failed to load history:', e);
      }
    }

    async function sendMessage() {
      const input = document.getElementById('chat-input');
      const text = input.value.trim();
      if (!text || !agentId) return;
      input.value = '';
      try {
        const res = await mcpRequest('tools/call', {
          name: 'chat_user_message',
          arguments: { agent_id: agentId, message: text }
        });
        renderMessages(parseToolJson(res));
      } catch (e) {
        console.error('Failed to send message:', e);
      }
    }

    document.getElementById('send-btn').addEventListener('click', sendMessage);
    document.getElementById('chat-input').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') sendMessage();
    });
    document.getElementById('clear-btn').addEventListener('click', async () => {
      if (!agentId) return;
      await mcpRequest('tools/call', {
        name: 'chat_clear',
        arguments: { agent_id: agentId }
      });
      renderEmpty();
    });

    // --- History refresh -------------------------------------------------
    // Replaces the old 1500 ms tools/call polling: `ui/context.refresh` via
    // `host/call` every 30 s (the host may answer with a fresh bootstrap, from
    // which we may learn an agent id for the first time), plus a history
    // re-read only when there is an agent to read, plus manual refresh.
    const CONTEXT_REFRESH_MS = 30000;
    const HISTORY_REFRESH_MS = 5000;
    let refreshing = false;

    async function manualRefresh() {
      if (refreshing) return;
      refreshing = true;
      const btn = document.getElementById('refresh-btn');
      if (btn) { btn.disabled = true; btn.textContent = '...'; }
      try {
        if (!agentId) await refreshContext();
        await loadHistory();
      } finally {
        refreshing = false;
        if (btn) { btn.disabled = false; btn.textContent = 'Refresh'; }
      }
    }

    document.getElementById('refresh-btn').addEventListener('click', manualRefresh);

    setInterval(() => { refreshContext(); }, CONTEXT_REFRESH_MS);
    setInterval(() => { if (agentId) loadHistory(); }, HISTORY_REFRESH_MS);

    // Request initial host context (the host also pushes ui/context).
    mcpRequest('ui/initialize', {}).then((ctx) => {
      updateContext(ctx);
      loadHistory();
    }).catch(() => {
      renderEmpty();
    });
  </script>
</body>
</html>"#;

/// 2. Pure MCP-Apps JSON-RPC Handler (`tools/call`, `resources/read`)
#[no_mangle]
pub extern "C" fn mcp_handle_request(
    method_ptr: *const u8,
    method_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i64 {
    let method = unsafe {
        core::str::from_utf8(core::slice::from_raw_parts(method_ptr, method_len)).unwrap_or("")
    };
    let params = unsafe {
        core::str::from_utf8(core::slice::from_raw_parts(params_ptr, params_len)).unwrap_or("")
    };

    match method {
        "resources/read" => {
            let uri = serde_json::from_str::<McpReadResourcePayload>(params)
                .map(|p| p.uri)
                .unwrap_or_else(|_| String::from("ui://chat-console"));

            if uri == "ui://chat-console" {
                let res = serde_json::json!({
                    "contents": [
                        {
                            "uri": "ui://chat-console",
                            "mimeType": "text/html;profile=mcp-app",
                            "text": CHAT_APP_HTML
                        }
                    ]
                });
                return pack_string_response(res.to_string());
            }

            let res = serde_json::json!({
                "contents": [
                    {
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": "[]"
                    }
                ]
            });
            pack_string_response(res.to_string())
        }

        "tools/call" | "mcp_call_tool" => {
            if let Ok(call) = serde_json::from_str::<McpCallToolPayload>(params) {
                match call.name.as_str() {
                    "chat_history" => {
                        let agent_id = call
                            .arguments
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim();
                        let key = format!("{}{}", HISTORY_KEY_PREFIX, agent_id);
                        let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                        let clean_json = if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&history_json) {
                            let valid: Vec<ChatMessage> = messages
                                .into_iter()
                                .filter(|m| !m.text.trim().is_empty())
                                .collect();
                            serde_json::to_string(&valid).unwrap_or(history_json)
                        } else {
                            history_json
                        };

                        let reply = serde_json::json!({
                            "content": [{ "type": "text", "text": clean_json }]
                        });
                        return pack_string_response(reply.to_string());
                    }

                    "chat_clear" => {
                        let agent_id = call
                            .arguments
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim();
                        let key = format!("{}{}", HISTORY_KEY_PREFIX, agent_id);
                        kv_delete(&key);
                        log(&format!("Cleared chat history for agent {}", agent_id));
                        let reply = serde_json::json!({
                            "content": [{ "type": "text", "text": r#"{"status":"cleared"}"# }]
                        });
                        return pack_string_response(reply.to_string());
                    }

                    "chat_user_message" => {
                        let agent_id = call
                            .arguments
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let trimmed_msg = call
                            .arguments
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim();

                        if trimmed_msg.is_empty() || agent_id.is_empty() {
                            let err = serde_json::json!({
                                "isError": true,
                                "content": [{ "type": "text", "text": "agent_id and non-empty message are required" }]
                            });
                            return pack_string_response(err.to_string());
                        }

                        let key = format!("{}{}", HISTORY_KEY_PREFIX, agent_id);
                        let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                        let mut messages: Vec<ChatMessage> =
                            serde_json::from_str(&history_json).unwrap_or_default();

                        let user_msg = ChatMessage {
                            id: format!("usr_{}", messages.len() + 1),
                            role: String::from("user"),
                            text: trimmed_msg.to_string(),
                            timestamp: get_current_time(),
                        };
                        messages.push(user_msg);

                        let updated_json = serde_json::to_string(&messages).unwrap_or_else(|_| String::from("[]"));
                        kv_set(&key, &updated_json);
                        kv_set(ACTIVE_AGENT_KEY, &agent_id);

                        let log_entry = format!("User {}: {}\n", agent_id, trimmed_msg);
                        fs_write(&transcript_path(&agent_id), log_entry.as_bytes());

                        unsafe {
                            host_agent_run_background(
                                agent_id.as_ptr(),
                                agent_id.len(),
                                trimmed_msg.as_ptr(),
                                trimmed_msg.len(),
                            );
                        }

                        let reply = serde_json::json!({
                            "content": [{ "type": "text", "text": updated_json }]
                        });
                        return pack_string_response(reply.to_string());
                    }

                    "chat_send" => {
                        let mut agent_id = call
                            .arguments
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let message = call
                            .arguments
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim();

                        if message.is_empty() {
                            let err = serde_json::json!({
                                "isError": true,
                                "content": [{ "type": "text", "text": "message is required and cannot be empty" }]
                            });
                            return pack_string_response(err.to_string());
                        }

                        if !agent_id.contains('-') || agent_id.starts_with("community.") {
                            if let Some(active) = kv_get(ACTIVE_AGENT_KEY) {
                                if !active.is_empty() {
                                    agent_id = active;
                                }
                            }
                        }

                        let key = format!("{}{}", HISTORY_KEY_PREFIX, agent_id);
                        let history_json = kv_get(&key).unwrap_or_else(|| String::from("[]"));
                        let mut messages: Vec<ChatMessage> =
                            serde_json::from_str(&history_json).unwrap_or_default();

                        let bot_msg = ChatMessage {
                            id: format!("ast_{}", messages.len() + 1),
                            role: String::from("assistant"),
                            text: message.to_string(),
                            timestamp: get_current_time(),
                        };
                        messages.push(bot_msg);

                        if let Ok(updated_json) = serde_json::to_string(&messages) {
                            kv_set(&key, &updated_json);
                            let log_entry = format!("Agent {} (chat_send): {}\n", agent_id, message);
                            fs_write(&transcript_path(&agent_id), log_entry.as_bytes());
                        }

                        let reply = serde_json::json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Message posted to chat console for agent '{}'.", agent_id)
                                }
                            ]
                        });
                        return pack_string_response(reply.to_string());
                    }

                    _ => {
                        let err = serde_json::json!({
                            "isError": true,
                            "content": [{ "type": "text", "text": format!("Unknown tool '{}'", call.name) }]
                        });
                        return pack_string_response(err.to_string());
                    }
                }
            }
            pack_string_response(String::from(
                r#"{"isError":true,"content":[{"type":"text","text":"Invalid payload for tools/call"}]}"#,
            ))
        }

        _ => pack_string_response(format!(
            r#"{{"isError":true,"content":[{{"type":"text","text":"Unsupported MCP method {}"}}]}}"#,
            method
        )),
    }
}
