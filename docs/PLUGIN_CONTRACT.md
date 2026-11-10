# NeuxDOT Plugin Contract

This is the author-facing spec for writing a **plugin** (an MCP server) the core
will load, sandbox, permission and drive. It documents the two halves a plugin
must satisfy:

1. **The MCP contract** — the JSON-RPC surface the host speaks (`initialize`,
   `tools/list`, `tools/call`, `resources/*`, `resources/subscribe` + push
   notifications).
2. **The permission & isolation contract** — the manifest, the permission
   object, and what each grant lets the host allow.

If you are building a **UI** (an "MCP-App") see the companion spec
[`HOST_BRIDGE.md`](./HOST_BRIDGE.md). Everything here is cited against the code;
the load-bearing files are listed in [§12](#12-where-the-contract-lives).

---

## 1. What a plugin is

A plugin is a row in `mcp_servers` (`migrations/0001_initial_schema.sql`). The
columns that matter to authors:

| Column | Meaning |
| --- | --- |
| `id` | Stable id (`wasm:<id>` for extensions). |
| `name` / `alias` | Display name; `alias` is the tool-namespace prefix. |
| `transport` | `stdio` \| `binary` \| `http` \| `custom_tools` \| `curl` \| `wasm`. |
| `url_or_command` | Executable+args (stdio/binary), base URL (http), or the tool manifest JSON (custom_tools). |
| `headers_or_env` | Env vars / CLI args (stdio) or HTTP headers (http); see §3.3. |
| `manifest_json` | The declarative manifest (JSON). Drives inferred permissions and UI. |
| `permissions_json` | The operator's **stored consent** — authoritative, see §6. |

An agent may bind zero or more plugins (`agent_mcp`), toggle individual tools
(`agent_tool_permissions`), and subscribe to push resources
(`agent_subscriptions`).

---

## 2. Transports

| Transport | How it runs | Process | Network default |
| --- | --- | --- | --- |
| `stdio` / `binary` | Native MCP server over stdio, **bubblewrap-sandboxed** | yes | **denied** |
| `http` | Remote MCP server over HTTP (+ SSE stream) | no | **unrestricted** |
| `custom_tools` / `curl` | Declarative HTTP tools evaluated in-process | no | **unrestricted** |
| `wasm` / `wasm_stdio` | Wasmtime extension served over an MCP shim | no (in-proc) | per manifest |

"binary" is stdio with a binary installed under `data/binaries/` (the upload
endpoint `POST /api/plugins/upload-binary` writes it atomically, so re-uploading
over a *running* binary never hits `ETXTBSY`). Point `url_or_command` at the
returned relative path.

Transport defaults, applied only when the manifest declares nothing
(`McpServer::declared_permissions`):

* **stdio / binary** → `process=true`, everything else denied (no network, no
  storage, no UI, no identity).
* **http** → `network=["*"]`, everything else denied.
* **custom_tools / curl** → `network=["*"]`, storage on (default
  `StoragePermissions`), no UI, no identity.
* **wasm** → per manifest.

---

## 3. The MCP contract

The host is a standard MCP **client** speaking JSON-RPC 2.0. Over stdio, each
message is one line of JSON on **stdout**; **stderr is for logs only** (it is
mirrored into the server log). The client advertises
`protocolVersion: "2024-11-05"`, `capabilities: { roots: { listChanged }, sampling }`.

### 3.1 Handshake

The host sends `initialize`, then `notifications/initialized`. Your server must
answer `initialize` within **2 seconds** (a non-standard server that times out is
tolerated — the host proceeds anyway). A typical `initialize` result:

```json
{ "protocolVersion": "2024-11-05",
  "capabilities": { "tools": { "listChanged": false },
                    "resources": { "subscribe": true } },
  "serverInfo": { "name": "my-plugin", "version": "1.0.0" } }
```

### 3.2 Tools

`tools/list` → `{ "tools": [ { "name", "description", "inputSchema" } ] }`.
`tools/call` params `{ name, arguments }` → a result of the form:

```json
{ "content": [ { "type": "text", "text": "…" } ], "isError": false }
```

Tools are **namespaced** `alias.tool_name` when presented to the model, and can
be resolved back from any of `alias.tool`, `alias__tool`, `alias:tool`, or a bare
`tool` when unique (`executor.rs::resolve_tool`). Set `_meta.ui.visibility` to
`["app"]` on a tool to make it callable **only** from the MCP-App UI, never by
the model.

### 3.3 Environment / arguments handed to your process

`headers_or_env` is parsed liberally:

* JSON object `{ "KEY": "value" }` → **environment variables**.
* JSON array `["--flag", "x"]` or a string starting with `-` → **CLI args**.
* Lines containing `=` or `:` → env vars (`KEY=value` or `Key: value`).

The host also exports **`NEUXDOT_AGENT_ID`** at spawn, so a plugin can resolve
per-agent state (tokens, channels) *before* the first tool call — required by
background streams.

### 3.4 Host-stamped caller identity

On every `tools/call` the host **overwrites** two underscore-prefixed arguments
(`mcp/mod.rs::stamp_sender`):

```json
{ "arguments": { …your schema…, "_sender_account_id": "…", "_sender_username": "…" } }
```

* Never put these in your `inputSchema` — they are host metadata, never shown to
  the model.
* You **cannot** be spoofed: the host rewrites them unconditionally, so trust
  them for attribution (who sent a message) but never as a capability grant.

### 3.5 Resources & MCP-Apps

* `resources/list` → `{ "resources": [ { "uri", "name", "mimeType" } ] }`.
* `resources/read` params `{ uri }` → `{ "contents": [ { "uri", "mimeType", "text" } ] }`.
* A `ui://` resource whose `text` is HTML is an **MCP-App** and is mounted in a
  sandboxed iframe (see [`HOST_BRIDGE.md`](./HOST_BRIDGE.md)). Declare it in the
  manifest so the `ui` grant is inferred.

### 3.6 Push subscriptions — the important one

For event streams (incoming chat, Telegram updates, webhooks) **do not poll
inside a tool call**. Instead:

1. The agent is subscribed to a resource URI (e.g.
   `extension://telegram/updates`), stored in `agent_subscriptions`.
2. The host keeps **one long-lived session per agent/plugin pair** alive while
   the agent is *idle* (`subscriptions.rs` — `SubscriptionHub`). It calls
   `resources/list` to see what you expose, then `resources/subscribe { uri }`.
3. When something happens, your server emits a notification:

```json
{ "jsonrpc": "2.0", "method": "notifications/resources/updated",
  "params": { "uri": "extension://telegram/updates", "text": "…", "from": "…" } }
```

The hub persists each notification (`agent_pushed_messages`), fans it out on the
event stream, and — if it looks like an **inbound user message** — wakes the idle
agent with a turn. The wake predicate (`is_inbound_user_message`) fires when
either `role == "user"` **or** the payload has a `from` **and** non-empty
`text`/`caption`. Outbound echoes must not carry `from`+text, or they will wake
the agent by its own traffic.

Why the hub (not the executor) owns subscriptions: a per-turn session would only
listen *during* a turn, miss messages while idle, and — for a bot API that allows
a single `getUpdates` consumer (Telegram answers `409` to a second) — let
superseded process pollers fight the new one.

### 3.7 Errors

Return a JSON-RPC `error` object `{ code, message }`. The host surfaces `message`
but **never** the request URL or your arguments. Do not put secrets in error
strings — they are logged (see §9).

---

## 4. The manifest

`manifest_json` is free-form but the host reads these keys:

```jsonc
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "description": "…",
  "mcp": {
    "tools":     [ { "name": "do_thing", "description": "…" } ],
    "resources": [ { "uri": "ui://my-app", "name": "App", "mimeType": "text/html" } ]
  },
  "permissions": { …see §5… },
  "network": { "hosts": ["api.example.com"] }   // legacy; prefer permissions.network
}
```

* `mcp.tools` / `mcp.resources` are used only to **infer** `agent.tools` and the
  `ui` grant. The authoritative tool list is still your `tools/list` at runtime.
* A legacy manifest with no `permissions` block: reachable hosts come from
  `network.hosts`, UI from `ui://` resources or a non-empty `ui_hooks` array,
  and identity is **never** inferred.

---

## 5. Permission reference

The permission object is the exact wire format of `PluginPermissions`
(`db/schema.rs`). Only the grouped shape deserialises; a missing section denies.
The full JSON example and the consent-resolution rules are in
[`HOST_BRIDGE.md §2`](./HOST_BRIDGE.md#2-permissions--consent). Per-grant meaning:

| Group | Field | Grants |
| --- | --- | --- |
| `ui` | `enabled` | May render `ui://` apps at all |
| `ui` | `resources` | Allow-list of `ui://` URIs (`["*"]`/`[]` = any) |
| `storage` | `get` / `set` / `delete` | Read / write / delete isolated KV |
| `storage` | `keys` | Optional list of key **prefixes** (`null`/`[]` = all) |
| `agent` | `trigger` | May start an agent turn (`agent.trigger` capability) |
| `agent` | `tools` | Its tools are exposed to the agent loop |
| `identity` | `agent.name` / `agent.profile_picture` / `user.name` / `user.profile_picture` | Read the matching display field (privacy-by-default: all `false`) |
| `process` | — | May spawn a child process (stdio/binary; auto-true for them) |
| `network` | `allow` | Outbound destinations; `[]` deny, `["*"]` allow-all, else allow-list |
| `elevated` | — | Run **on the host**, no sandbox (manifest-only, deny-by-default) |
| `control` | 8 flags | Drive the server's own control API, per resource (§5.3) |

### 5.1 Network allow-list

`network.allow` entries may be an exact hostname, a `*.suffix` / `.suffix`
wildcard ("this domain and every subdomain"), an IP literal, or a CIDR block.
Matching is case-insensitive; `api.example.com.evil.test` does **not** match
`api.example.com`. Everything in between `[]` and `["*"]` is enforced by a
host-side CONNECT proxy bound into the sandbox over a unix socket (§7). If your
sandboxed plugin needs the internet, **you must list the host** — the default is
no route at all.

### 5.2 Capability strings

The subset of grants that reach the **UI** are named capabilities
(`ui.context.refresh`, `storage.*`, `agent.trigger`, `identity.*`); the mapping
from permission → capability is in `PluginPermissions::capabilities()`. See
[`HOST_BRIDGE.md §1`](./HOST_BRIDGE.md#1-capabilities).

### 5.3 Control grants

`permissions.control` lets a plugin call the server's own control MCP
(`POST /api/control/mcp`). It is deny-by-default and per resource:
`agents_read`, `agents_write`, `shares`, `plugins_read`, `plugins_write`,
`providers_read`, `providers_write`, `accounts`. Two independent gates apply: the
HTTP Basic credential's own account **and** the granted scopes. `accounts`
additionally requires a `root` credential. Secrets are never returned
(`api_key` → `has_api_key: true`; `headers_or_env` is never echoed).

---

## 6. Consent resolution

Effective grants = `McpServer::get_permissions()`:

1. **Stored consent** (`permissions_json`) — the operator's decision, wins over
   the manifest. Only a *grouped* override counts; a legacy flat override is
   ignored (falls back to declared) rather than silently denying everything.
2. **Manifest** `permissions` block (grouped only; unparseable ⇒ deny all).
3. **Inferred** from `ui://` resources / `mcp.tools` / `network.hosts`.
4. **Transport defaults** (§2).

`elevated` may be revoked by a stored override but never added by one — only the
manifest can request host execution, and the consent UI shows an amber warning
before install.

---

## 7. Sandbox & process contract (stdio / binary)

Unless `elevated`, a stdio/binary plugin runs under `bwrap --unshare-all`
(`mcp/sandbox.rs`): its own user/pid/net/ipc/uts/cgroup/mnt namespaces.

| Resource | What the plugin sees |
| --- | --- |
| Filesystem | Read-only `/usr` (with `/bin`,`/lib`,`/lib64` symlinks); **no `/home`** |
| `/etc` | Read-only `ssl`, `ca-certificates`, `resolv.conf`, `hosts`, `nsswitch.conf`, `passwd`, `group`, `ld.so.cache` |
| `/tmp` | A private tmpfs |
| Workspace | `$HOME` = `/workspace`, the **only** writable path; `cwd` is here |
| Network | **Nothing** unless the manifest allow-lists destinations |

Env injected: `HOME=/workspace`, `TMPDIR=/tmp`,
`PATH=/usr/local/bin:/usr/bin:/bin`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, plus
any `headers_or_env` and `NEUXDOT_AGENT_ID`.

### Network path

When `network.allow` is a non-empty non-wildcard list, the host starts an
`AllowListProxy` on a unix socket, binds it into the sandbox, and runs a `socat`
relay inside the sandbox namespace to `127.0.0.1:41873`. The plugin receives:

```
HTTP_PROXY=HTTPS_PROXY=ALL_PROXY=http://127.0.0.1:41873   (upper + lower case)
NO_PROXY=no_proxy=""   (empty, so the proxy stays in charge of "localhost")
```

**Your HTTP client must honour proxy env vars** (reqwest does by default; so does
`curl`). A client that opens raw TCP cannot reach anything — the sandbox has no
route except loopback, and loopback is the proxy. The proxy parses
`CONNECT host:port` (TLS) or absolute-form (plain HTTP), checks `allows_host`,
and forwards bytes without inspecting them (it is **not** a TLS interceptor).

Implications for plugin authors:

* Honour `HTTP(S)_PROXY`/`ALL_PROXY`; don't hard-code direct connections.
* Persist state under `/workspace`, not `/tmp` (tmpfs is wiped) and not `/home`
  (does not exist).
* Don't daemonise: the relay is `socat … &`, but *your* process is the MCP loop.
  Do long work inside `resources/subscribe` streams, not as detached background
  processes.
* If you truly need host filesystem/network with no isolation, ask for
  `permissions.elevated=true` and say why — it is the single most dangerous flag.

---

## 7.5 The Server Control plugin (auto-installed)

The core seeds a `Server Control` plugin per account at boot
(`db/seed.rs::ensure_control_plugins`), bound to that account's agents:

| Field | Value |
| --- | --- |
| id | `control-mcp-<account_id>` (deterministic) |
| alias | `server` |
| transport | `http` → `http://127.0.0.1:<PORT>/api/control/mcp` |
| `headers_or_env` | **`NULL`** — no shared credential |
| `permissions.control` | agents/shares/plugins/providers read+write; `accounts` **false** |
| tools | `server_*` (alias `server`) — `server_create_agent`, `server_list_plugins`, … |

Seeding is idempotent: re-boot refreshes only the loopback URL (so a `PORT`
change does not strand the endpoint) and never overwrites an operator's
`permissions_json` edit.

**Credentials are per agent.** Every call is authenticated by HTTP Basic against
a real account. The shared row has no header, so each agent must set
`Authorization: Basic base64(user:pass)` through `agent_mcp.config_override`
(the agent's plugin **Auth / Config** button). `merge_headers_or_env` then
supplies it per spawn. Without an override the endpoint returns
`missing Authorization header`, so a seeded plugin is inert until an operator
configures it — least privilege by default.

Because the credential *is* the authority, two gates both apply: the account's
own role/sharing (enforced by the repositories) and the plugin's declared
`control` scopes (only granted tools are advertised).

---

### 7.6 Tool naming

The model-facing name is `<alias>.<tool>`, but a plugin whose tools are already
prefixed with its own alias does **not** repeat it:

```
plugin alias "telegram", tool "telegram_send_message"  ->  telegram.send_message
plugin alias "control",  tool "control_whoami"         ->  control.whoami
plugin alias "chat",     tool "send"                   ->  chat.send
```

Implementing this is `compose_tool_name()` in `runtime/executor.rs`. The
redundant `<alias>_` prefix is the only thing stripped; a tool that merely
*starts* with the alias (`alias "a"`, tool `a_b`) is left intact, and
`resolve_tool` still accepts the bare original name.

`search_tools` distinguishes "no such tool" from "this plugin could not be
enumerated": if any bound plugin failed `tools/list` during the turn, the result
carries `plugins_unavailable: [...]` and a note instead of a bare empty list.

---

### 7.7 One turn per agent, and batching

An agent runs **at most one turn at a time** (`runtime/turn_queue.rs`). The HTTP
route, the MCP push bridge and the `SubscriptionHub` may all try to start one;
only the first becomes the runner.

* A message arriving mid-turn is appended to a pending queue.
* When the running turn yields (a `sleep` is interrupted, or the loop reaches an
  iteration boundary), it stops and the runner drains the queue.
* Two or more queued messages are merged into one prompt:
  `"N messages arrived while you were working. Address them together in one reply: 1. … 2. …"`.
  A single queued message passes through unwrapped.
* Ownership is a token, released by a drop guard, so a turn that unwinds cannot
  leave the agent locked.

Without this, two concurrent turns each call `sleep`, each `register_timer`
cancels the other, and the agent ping-pongs on `cancelled: superseded by new
timer` forever.

---

## 8. Lifecycle inside the host

1. **Install** — `POST /api/plugins` (aka `/api/mcp`, `/api/extensions`) stores
   the row; binary uploads land in `data/binaries/`.
2. **Bind** — attach the plugin to an agent (`agent_mcp`); per-tool toggles live
   in `agent_tool_permissions`.
3. **Discover** — the executor calls `tools/list` and registers each tool as
   `alias.tool`; the model reaches them through the invariant `search_tools` /
   `get_tool_schema` / `call_tool` meta-tools (JIT — schemas are not injected up
   front).
4. **Subscribe** — `SubscriptionHub` reconciles one long-lived session per
   agent/plugin with subscriptions, at boot and on any change.
5. **Shutdown** — sessions close on unsubscribe/unbind/shutdown.

Authorization is checked on every RPC: the caller must own the plugin **or** be
able to view an agent the plugin is bound to (shared bots). Prefer per-agent
overrides (`agent_mcp.config_override`) over the plugin row's own config.

---

## 9. Security rules for authors

* **Never log secrets.** A bot token in a request URL will land in the host log
  verbatim. Redact before logging; treat `user_message()`-style errors as
  untrusted. (A leaked Telegram token is a real incident — rotate via BotFather.)
* **Never echo `headers_or_env`** or credentials in tool output.
* **Trust `_sender_*`** for attribution, but never treat it as a grant.
* **Stay in `/workspace`.** No host paths, no `/proc` escapes.
* **Least privilege.** Ask only for the network hosts you need and the storage
  prefixes you use. Default to no `elevated`, no `["*"]` network.
* **Omit, don't blank.** When a grant is absent, leave the data out entirely
  (this is the host's own policy in `HOST_BRIDGE.md §3`; mirror it).

---

## 10. Minimal plugin skeleton (Rust, `rmcp`)

```rust
mod server; // implements the MCP server (tools/list, tools/call, resources/*)

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout carries the MCP stream; logs go to stderr only.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = server::MyPlugin::new()
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
```

Manifest to ship alongside it:

```json
{
  "id": "my-plugin", "name": "My Plugin", "version": "1.0.0",
  "permissions": {
    "process": true,
    "network": { "allow": ["api.example.com"] },
    "ui": { "enabled": true, "resources": ["ui://my-app"] },
    "storage": { "get": true, "set": true, "delete": false, "keys": ["myplugin."] }
  },
  "mcp": {
    "tools":     [{ "name": "do_thing", "description": "Does the thing" }],
    "resources": [{ "uri": "ui://my-app", "name": "My App", "mimeType": "text/html" }]
  }
}
```

Then: `./compile.sh --plugins` (copies built-ins into `release/plugins/`) or
`POST /api/plugins/upload-binary` for a one-off binary.

---

## 11. Acceptance checklist

- [ ] Answers `initialize` < 2s; writes JSON-RPC only to stdout, logs to stderr.
- [ ] `tools/list` returns stable names; `tools/call` returns
      `{ content:[{type:"text",text}], isError }`.
- [ ] Declares tools with `_meta.ui.visibility:["app"]` where UI-only.
- [ ] Honours `HTTP(S)_PROXY`; only reaches hosts listed in `network.allow`.
- [ ] Persists under `/workspace`; survives a tmpfs wipe.
- [ ] Implements `resources/subscribe` for streams; emits
      `notifications/resources/updated`; outbound echoes carry no `from`+text.
- [ ] Never logs or returns `headers_or_env` / credentials.
- [ ] Manifest asks for the minimum grants; `elevated` only if unavoidable.
- [ ] `ui://` HTML follows `HOST_BRIDGE.md` (opaque origin, `host/call`, omission).

---

## 12. Where the contract lives

| Concern | File |
| --- | --- |
| Permission object, capabilities, manifest inference | `src/core/neuxdot-core/src/db/schema.rs` |
| Stdio transport, handshake, env/args, `NEUXDOT_AGENT_ID` | `src/core/neuxdot-core/src/mcp/stdio.rs` |
| HTTP/SSE transport | `src/core/neuxdot-core/src/mcp/http.rs` |
| Declarative custom tools | `src/core/neuxdot-core/src/mcp/custom_tools.rs` |
| Sandbox + network proxy | `src/core/neuxdot-core/src/mcp/sandbox.rs`, `mcp/proxy.rs` |
| Caller-identity stamping, host metadata keys | `src/core/neuxdot-core/src/mcp/mod.rs` |
| RPC entry point, per-resource UI check, pushes | `src/core/neuxdot-server/src/routes/mcp.rs` |
| Long-lived push subscriptions / wake predicate | `src/core/neuxdot-server/src/subscriptions.rs` |
| Host bridge endpoints (UI) | `src/core/neuxdot-server/src/routes/host.rs` |
| UI bridge spec | `docs/HOST_BRIDGE.md` |
| DB tables | `src/core/neuxdot-core/migrations/0001_initial_schema.sql` |
| Reference plugins | `built-in-plugin/{chat,computer,telegram,discord,gitlab}` |
