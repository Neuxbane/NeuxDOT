# Host ↔ MCP-App Bridge Contract

Status: **normative** for `McpAppIframe.svelte`, `api.ts`, `routes/host.rs`, the `/mcp`
permission editor and every example plugin. If code and this document disagree, the
document wins — raise it with the Lead instead of silently diverging.

## 0. Why this exists

Two runtime failures motivated the redesign:

| Symptom | Root cause |
| --- | --- |
| `Uncaught DOMException: Proxy object could not be cloned` at `sendContextToIframe` | The host `postMessage`d Svelte 5 `$state` **proxies** (`agent`, `permissions`, `authState` getters). Structured clone cannot serialise proxies. |
| `Security Error: Content at http://localhost:5173/mcp may not load or link to file:///` | The app runs in `srcdoc` + `sandbox="allow-scripts"`, i.e. an **opaque `null` origin**. It cannot resolve relative URLs, cannot reliably load `data:`/`file:` picture values, and must not be given `allow-same-origin`. |

Consequences that shape everything below:

1. Only **structurally cloned primitive data** ever crosses `postMessage`.
2. The app never *fetches* anything for identification. Avatars, names and storage
   values are **injected by the host** as plain values, resolved **server-side** through
   the authenticated API (`/api/host/*`). This is the "API proxy" path: the browser does
   not read the profile, the server does, and the server is the authority on consent.

## 1. Capabilities

A capability is a dotted, namespaced **grant**. Only granted capabilities appear in the
bootstrap. A capability that is absent is *not granted* — the app must degrade, never
guess.

| Capability | Grant that enables it | What crosses the boundary |
| --- | --- | --- |
| `ui.context.refresh` | `ui.enabled = true` | none (re-fetch of this payload) |
| `storage.get` | `storage.get = true` (+ `storage.keys` prefix) | stored value for an allowed key |
| `storage.set` | `storage.set = true` (+ `storage.keys` prefix) | key/value written to isolated storage |
| `storage.delete` | `storage.delete = true` (+ `storage.keys` prefix) | key removed |
| `storage.keys` | `storage.get = true` | key names inside the allowed prefixes |
| `agent.trigger` | `agent.trigger = true` | free-text prompt forwarded to the agent |
| `identity.agent.name` | `identity.agent.name = true` | agent display name |
| `identity.agent.profile_picture` | `identity.agent.profile_picture = true` | agent avatar URL/colour/shape |
| `identity.user.name` | `identity.user.name = true` | signed-in user display name |
| `identity.user.profile_picture` | `identity.user.profile_picture = true` | user avatar URL/colour/shape |

## 2. Permission object (`PluginPermissions`)

Canonical shape. There is exactly one accepted shape — the flat boolean blob of
earlier releases has been removed (see "No flat fallback" below):

```json
{
  "ui": { "enabled": true, "resources": ["ui://chat-console"] },
  "storage": { "get": true, "set": true, "delete": true, "keys": ["chat."] },
  "agent": { "trigger": true, "tools": true },
  "identity": {
    "agent.name": false,
    "agent.profile_picture": true,
    "user.name": true,
    "user.profile_picture": false
  },
  "process": false,
  "network": false
}
```

* `ui.resources` is an allow-list of `ui://` URIs. `["*"]` is the wildcard marker
  meaning "every `ui://` resource of this plugin".
* `storage.keys` is a list of **key prefixes**; `null` means "all keys".
* `process` / `network` stay flat booleans for now (out of scope for this pass) but are
  **not** shown in the consent editor.
* **Privacy by default:** a plugin that does not ask for identity grants gets *none* of
  them. This is a behaviour change from the old `identity: true` default.

### No flat fallback

The flat boolean blob `{ ui, storage, process, network, agent_control, identity }`
of earlier releases is **removed**. It no longer deserialises on the Rust side and
is not accepted by the frontend: a plugin declaring it is treated as requesting
nothing, and storage rows written in that shape must be re-saved. Dropping it
removes an entire class of "did the default mean yes?" ambiguity from the consent
model.

## 3. Bootstrap payload

Pushed on `ui/context` and returned by `ui/initialize`:

```json
{
  "protocolVersion": "2025-03-26",
  "app": {
    "pluginId": "wasm:1f0c…",
    "pluginName": "Agent Chat Console",
    "resourceUri": "ui://chat-console",
    "title": "Agent Chat Console"
  },
  "agent": {
    "id": "8b1e…",
    "name": "Researcher",
    "avatar": { "url": "https://…", "color": "#3b82f6", "shape": "circle" }
  },
  "user": {
    "name": "Ada",
    "avatar": { "url": "data:image/png;base64,…", "color": "#10b981", "shape": "rounded" }
  },
  "capabilities": ["ui.context.refresh", "identity.agent.name", "identity.user.name"],
  "permissions": { "…": "the resolved PluginPermissions object" },
  "context": { "locale": null, "theme": "dark" }
}
```

**Omission rule.** Anything not granted is *absent*, not blanked and not replaced by a
placeholder. There is no `"AI Agent"` / `"User"` fallback string anywhere in the host —
the app decides its own placeholder when `agent?.name` is `undefined`.

## 4. Transport

Host → app (JSON-RPC 2.0 **notification**):

```js
{ jsonrpc: "2.0", method: "ui/context", params: <bootstrap> }
```

App → host (JSON-RPC 2.0 **request**, `id` echoed back):

```js
{ jsonrpc: "2.0", id: 7, method: "host/call",
  params: { capability: "storage.get", args: { key: "chat.draft" } } }
```

Response on success: `{ jsonrpc:"2.0", id:7, result:{ ok:true, value: <any> } }`

Errors:

| Code | Meaning |
| --- | --- |
| `-32004` | capability not granted (or key outside `storage.keys`) |
| `-32000` | anything else (transport/validation failure) |

`tools/call`, `resources/read`, `resources/list` keep proxying to
`POST /api/plugins/:id/rpc` exactly as today. `ui/message` remains a supported
notification that triggers an agent turn **only** with `agent.trigger`.

## 5. Host endpoints (server-side authority)

| Endpoint | Purpose |
| --- | --- |
| `GET /api/host/session?plugin_id=&agent_id=&resource_uri=` | Builds the bootstrap server-side from the session, agent row and profile row. 403 if the plugin/agent belongs to another account. |
| `GET /api/host/avatar?kind=user|agent&id=` | Streams the stored avatar bytes (decoding `data:` URLs) or redirects `http(s)`. Lets the sandboxed app show a picture without linking to an arbitrary scheme. |
| `POST /api/host/storage/{get,set,delete,list}` | `{ plugin_id, key?, value?, prefix? }` — isolated per `(account_id, plugin_id)`, enforcing the `storage.*` grants and the key prefixes. |

## 6. TypeScript surface (`$lib/api`)

```ts
type Capability =
  | 'ui.context.refresh'
  | 'storage.get' | 'storage.set' | 'storage.delete' | 'storage.keys'
  | 'agent.trigger'
  | 'identity.agent.name' | 'identity.agent.profile_picture'
  | 'identity.user.name' | 'identity.user.profile_picture';

interface PluginPermissions {
  ui: { enabled: boolean; resources: string[] };
  storage: { get: boolean; set: boolean; delete: boolean; keys: string[] | null };
  agent: { trigger: boolean; tools: boolean };
  identity: {
    'agent.name': boolean;
    'agent.profile_picture': boolean;
    'user.name': boolean;
    'user.profile_picture': boolean;
  };
  process: boolean;
  network: boolean;
}

interface HostBridgeIdentity {
  agent?: { id: string; name?: string; avatar?: { url?: string; color?: string; shape?: string } };
  user?: { name?: string; avatar?: { url?: string; color?: string; shape?: string } };
}

interface HostBridgeBootstrap {
  protocolVersion: string;
  app: { pluginId: string; pluginName: string; resourceUri: string; title: string };
  agent?: HostBridgeIdentity['agent'];
  user?: HostBridgeIdentity['user'];
  capabilities: Capability[];
  permissions: PluginPermissions;
  context: { locale: string | null; theme: 'light' | 'dark' };
}
```

## 7. Consuming it app-side

```js
const ctx = await rpc('ui/initialize', {});          // or wait for the ui/context push
header.name.textContent = ctx.agent?.name ?? 'Chat'; // omitted → our own neutral label
if (ctx.capabilities.includes('identity.agent.profile_picture')) {
  paintAvatar(ctx.agent.avatar);                     // only if granted
}
const { value } = await rpc('host/call', { capability: 'storage.get', args: { key: 'chat.draft' } });
```

Rendering rules for apps:

* Build avatars with `createElement` + `textContent`, never by interpolating user
  strings into `innerHTML`.
* Only emit `<img>` when the URL starts with `https:`, `http:` or `data:image/`; always
  keep an `onerror` fallback to the coloured initial.
* Never render the literal text `undefined`.

## 8. Sandboxing rules (non-negotiable)

* The iframe keeps `sandbox="allow-scripts"`. **Do not add `allow-same-origin`** — that
  would hand the app the host origin's `localStorage`, cookies and DOM.
* `event.origin` is the string `"null"` for such an iframe, so it cannot be used to
  authenticate the sender. The real check is `event.source === iframe.contentWindow`.
* Every `postMessage` goes through a helper that structurally clones and is wrapped in
  `try/catch`, so a mistake surfaces in the console instead of as an uncaught
  `DOMException`.
