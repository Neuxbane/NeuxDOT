# Host Bridge — sandboxed MCP-App ↔ host contract

A plugin can ship a **UI** (an "MCP-App") as a `ui://` resource. The host mounts
it in an `<iframe sandbox="allow-scripts" srcdoc="…">`, i.e. an **opaque
origin**. It cannot read cookies, storage, relative URLs or the profile. Anything
it is allowed to know is resolved **server-side against the authenticated
session** and pushed in over `postMessage`.

This file is the normative spec. Code that cites `docs/HOST_BRIDGE.md §N`
refers to the sections below. The Rust half lives in
`src/core/neuxdot-server/src/routes/host.rs`, the browser half in
`src/apps/web/src/lib/components/McpAppIframe.svelte`, and the permission shape
in `src/core/neuxdot-core/src/db/schema.rs`.

The wire protocol version is **`2025-03-26`** (`HOST_BRIDGE_PROTOCOL_VERSION`).

---

## §1 Capabilities

A capability is the smallest grant an app can be given. The host derives the
grant list from the plugin's resolved permissions and ships only the **granted**
ones in `bootstrap.capabilities`. Canonical strings
(`db/schema.rs: CAP_*`):

| Capability | Granted when | What it lets the app do |
| --- | --- | --- |
| `ui.context.refresh` | `ui.enabled` | Ask the host to re-resolve the bootstrap |
| `storage.get` | `storage.get` | Read a key in its own namespace |
| `storage.keys` | `storage.get` | List keys in its own namespace |
| `storage.set` | `storage.set` | Write a key |
| `storage.delete` | `storage.delete` | Delete a key |
| `agent.trigger` | `agent.trigger` | Start an agent turn with the app's text |
| `identity.agent.name` | `identity.agent.name` | Read the agent display name |
| `identity.agent.profile_picture` | `identity.agent.profile_picture` | Read the agent avatar |
| `identity.user.name` | `identity.user.name` | Read the viewer's display name |
| `identity.user.profile_picture` | `identity.user.profile_picture` | Read the viewer's avatar |
| `identity.resolve` | `identity.user.name` **or** `identity.user.profile_picture` | Resolve name/avatar for account ids the app already stores |

`identity.resolve` is derived: if the app may see its own user's name or avatar,
it may also resolve the same non-sensitive fields for other authors it already
references (e.g. chat message senders). There is **no enumeration** — the app
must already know the ids.

---

## §2 Permissions & consent

The canonical permission object is the exact wire format of
`PluginPermissions` (`db/schema.rs`). Only the **grouped** shape deserialises;
a missing section defaults to *not granted*; an unparseable block denies
everything. The same shape is mirrored in `apps/web/src/lib/utils/permissions.ts`
— keep them in lock-step.

```jsonc
{
  "ui":       { "enabled": true,  "resources": ["ui://my-app"] },
  "storage":  { "get": true, "set": true, "delete": false, "keys": ["myapp."] },
  "agent":    { "trigger": true, "tools": true },
  "identity": { "agent.name": true, "agent.profile_picture": false,
                "user.name": true,  "user.profile_picture": false },
  "process":  false,
  "network":  { "allow": ["api.example.com", "*.example.org"] },
  "elevated": false,
  "control":  { "agents_read": false, "agents_write": false, "shares": false,
                "plugins_read": false, "plugins_write": false,
                "providers_read": false, "providers_write": false,
                "accounts": false }
}
```

Resolution order (`McpServer::get_permissions`):

1. **Stored consent** — `mcp_servers.permissions_json`, written by the /mcp
   consent editor. This is the operator's decision and **wins** over the
   manifest. A plugin can never grant itself access nobody approved.
2. **Manifest `permissions` block** — parsed verbatim.
3. **Inferred defaults** — from `ui://` resources, `mcp.tools`, and the legacy
   `network.hosts` array.
4. **Transport defaults** — see the table in `PLUGIN_CONTRACT.md`.

`elevated` is special: a stored override may **revoke** it but never invent it.
It only comes from the manifest.

### Per-resource UI allow-list

`ui.resources` is an allow-list of `ui://` URIs. `["*"]` (or an empty list)
means "any". `resources/read` on a `ui://` URI is refused with `403` unless
`allows_ui_resource(uri)` passes (`routes/mcp.rs`). The server enforces this; the
iframe grant check is defence in depth only.

### Storage key prefixes

`storage.keys` is an optional list of **prefixes**. `null`/`[]` means "every
key". Every get/set/delete/list is checked against both the verb grant and the
prefix allow-list (`check_storage_access`). A listed key that falls outside the
prefixes is filtered out of `storage.list`.

---

## §3 Bootstrap & the omission rule

`GET /api/host/session?plugin_id=…&agent_id=…&resource_uri=…&theme=dark|light`
returns `HostBridgeBootstrap`:

```jsonc
{
  "protocolVersion": "2025-03-26",
  "app": { "pluginId": "wasm:…", "pluginName": "…",
           "resourceUri": "ui://my-app", "title": "…" },
  "agent": { "id": "…", "name": "…",
             "avatar": { "url": "…", "color": "…", "shape": "…" } },   // optional
  "user":  { "id": "…", "name": "…", "avatar": { … } },                 // optional
  "capabilities": ["ui.context.refresh", "storage.get", …],
  "permissions": { …the resolved PluginPermissions above… },
  "context": { "locale": null, "theme": "dark" }
}
```

**Omission rule (normative).** A value is emitted only when its grant is
present, and **omitted entirely** rather than blanked:

* `agent.name` is absent unless `identity.agent.name`; `agent.avatar` is absent
  unless `identity.agent.profile_picture`. If neither is granted, the whole
  `agent` block is omitted.
* Same for `user.name` / `user.avatar` and the `user` block.
* `agent.id` is always present so the app can key its own state; it is not
  personally identifying.

Bootstrap is delivered three ways: pushed on iframe `load` (`ui/context`),
returned from `ui/initialize`, and returned by `host/call` for
`ui.context.refresh`. A plugin that never calls `ui/initialize` still renders
because the host pushes `ui/context` on load.

---

## §4 `host/call` RPC methods

The iframe speaks JSON-RPC 2.0 over `postMessage`. Requests carry an `id`;
notifications do not. Every message must pass `event.source === iframe.contentWindow`
(the opaque origin makes `event.origin` useless as an authenticator — never add
`allow-same-origin`).

### Host-implemented methods

| Method | Params | Result |
| --- | --- | --- |
| `ui/initialize` | — | the bootstrap |
| `host/call` | `{ capability, args }` | `{ ok: true, value }` |
| notifications | | |
| `ui/message` | `{ text }` | starts an agent turn (needs `agent.trigger`) |

`host/call` dispatches on `capability`, after a grant check (un-granted →
`-32004`):

| `capability` | `args` | `value` |
| --- | --- | --- |
| `ui.context.refresh` | — | fresh bootstrap |
| `identity.agent.name` / `…profile_picture` | — | string / avatar |
| `identity.user.name` / `…profile_picture` | — | string / avatar |
| `identity.resolve` | `{ ids: string[] }` | `{ [id]: { id, username, name, avatar } }` |
| `agent.trigger` | `{ text }` | the run result (`text` must be non-empty) |
| `storage.get` | `{ key }` | `{ key, value }` |
| `storage.set` | `{ key, value }` | `{ ok, key }` (non-strings are JSON-stringified) |
| `storage.delete` | `{ key }` | `{ ok }` |
| `storage.keys` | `{ prefix? }` | `{ keys: string[] }` |

### Forwarded MCP methods

These are proxied to the plugin's own MCP transport (`POST /api/plugins/:id/rpc`),
i.e. `resources/read` returns the plugin's `ui://` HTML, `tools/call` runs the
plugin's tools:

`tools/call`, `resources/read`, `resources/list`, `resources/subscribe`

### Host → app notifications

| Method | Params | Meaning |
| --- | --- | --- |
| `ui/context` | bootstrap | (Re)send the resolved context |
| `ui/history` | `{ revision }` | Persisted history changed; refetch |

---

## §5 Host HTTP endpoints (server-side, session-authenticated)

| Endpoint | Purpose |
| --- | --- |
| `GET /api/host/session` | Build the bootstrap |
| `GET /api/host/avatar?kind=user\|agent&id=` | Stream an avatar (data: decoded, http→302) |
| `GET /api/host/identities?ids=a,b,c` | Resolve display data for known account ids (≤100) |
| `POST /api/host/storage/get\|set\|delete\|list` | Isolated KV; body `{ plugin_id, agent_id?, key?, value?, prefix? }` |

**Storage namespacing.** The namespace is the plugin's extension id (`wasm:` is
stripped). The host imposes **no** tenancy beyond that — per-agent or per-account
partitioning is the *plugin's* job, expressed in the keys it chooses (the chat
extension uses `chat.history.<agent_id>`). A sharee may reach a plugin's storage
only when they can view an agent the plugin is attached to.

---

## §6 Security invariants

* The iframe is `sandbox="allow-scripts"` with an **opaque origin**. Never add
  `allow-same-origin`, `allow-forms`, or `allow-popups`.
* Authenticate messages by `event.source`, never by `event.origin`.
* Consent is authoritative server-side. Ungranted data is **omitted**, not
  blanked. The browser-side grants are defence in depth only.
* A `file:`, relative, `blob:` or `data:` link is never handed to the iframe for
  avatars; the host streams bytes or redirects to `http(s)`.
* If `/api/host/session` is unreachable the client falls back to a bootstrap
  that **grants nothing** and ships no identity — a backend blip can never leak
  profile data.
