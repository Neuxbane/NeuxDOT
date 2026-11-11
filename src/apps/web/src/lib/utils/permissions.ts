import type { ControlPermissions, McpServer, PluginPermissions } from '$lib/api';

/**
 * Permission helpers shared by every surface that renders or edits plugin
 * grants. The canonical shape is defined in `docs/HOST_BRIDGE.md` §2 — keep this
 * file in lock-step with the Rust `PluginPermissions` deserializer in
 * `crates/neuxdot-core/src/db/schema.rs`. Only the grouped shape is supported.
 */

/** A grant set where nothing at all is allowed. */
export function denyAll(): PluginPermissions {
return {
ui: { enabled: false, resources: [] },
storage: { get: false, set: false, delete: false, keys: [] },
agent: { trigger: false, tools: false },
identity: {
'agent.name': false,
'agent.profile_picture': false,
'user.name': false,
'user.profile_picture': false
},
process: false,
network: { allow: [] },
elevated: false,
control: {
agents_read: false,
agents_write: false,
shares: false,
plugins_read: false,
plugins_write: false,
providers_read: false,
providers_write: false,
accounts: false
}
};
}

function isRecord(v: unknown): v is Record<string, any> {
return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/**
 * Coerce a stored/declared permission object into a complete canonical shape.
 *
 * Only the grouped shape is accepted; a partial object is filled with *denied*
 * defaults so a missing section can never silently grant access. The flat
 * boolean blob of earlier releases is deliberately unsupported.
 */
function normalizePermissions(input: unknown): PluginPermissions {
const out = denyAll();
if (!isRecord(input)) return out;

if (isRecord(input.ui)) {
out.ui = {
enabled: input.ui.enabled === true,
resources: Array.isArray(input.ui.resources)
? input.ui.resources.filter((r: unknown): r is string => typeof r === 'string')
: []
};
}
if (isRecord(input.storage)) {
out.storage = {
get: input.storage.get === true,
set: input.storage.set === true,
delete: input.storage.delete === true,
keys: Array.isArray(input.storage.keys)
? input.storage.keys.filter((k: unknown): k is string => typeof k === 'string')
: null
};
}
if (isRecord(input.agent)) {
out.agent = { trigger: input.agent.trigger === true, tools: input.agent.tools === true };
}
if (isRecord(input.identity)) {
const id = input.identity;
out.identity = {
'agent.name': id['agent.name'] === true,
'agent.profile_picture': id['agent.profile_picture'] === true,
'user.name': id['user.name'] === true,
'user.profile_picture': id['user.profile_picture'] === true
};
}
out.process = input.process === true;
out.network = normalizeNetwork(input.network);
out.elevated = input.elevated === true;
out.control = normalizeControl(input.control);
return out;
}

/**
 * Coerce a stored/declared `network` grant into `{ allow: string[] }`.
 *
 * Accepts the current grouped form (`{ allow: ["api.example.com"] }`) and the
 * legacy boolean of earlier releases, where `true` meant "unrestricted" and
 * `false` meant "no network". Anything unrecognised denies network outright.
 */
function normalizeNetwork(input: unknown): { allow: string[] } {
if (input === true) return { allow: ['*'] };
if (input === false || input == null) return { allow: [] };
if (isRecord(input) && Array.isArray(input.allow)) {
return {
allow: input.allow.filter((h: unknown): h is string => typeof h === 'string' && h.length > 0)
};
}
return { allow: [] };
}

/**
 * Coerce a stored/declared `control` grant. Every flag is denied unless it is
 * literally `true`, so a malformed block narrows rather than widens.
 */
function normalizeControl(input: unknown): ControlPermissions {
const deny: ControlPermissions = {
agents_read: false,
agents_write: false,
shares: false,
plugins_read: false,
plugins_write: false,
providers_read: false,
providers_write: false,
accounts: false
};
if (!isRecord(input)) return deny;
return {
agents_read: input.agents_read === true,
agents_write: input.agents_write === true,
shares: input.shares === true,
plugins_read: input.plugins_read === true,
plugins_write: input.plugins_write === true,
providers_read: input.providers_read === true,
providers_write: input.providers_write === true,
accounts: input.accounts === true
};
}

/** The `ui://` resources a manifest advertises. */
function manifestUiResources(manifest: any): string[] {
const res = manifest?.mcp?.resources;
if (!Array.isArray(res)) return [];
return res
.filter((r: any) => typeof r?.uri === 'string' && r.uri.startsWith('ui://'))
.map((r: any) => r.uri as string);
}

/**
 * Legacy reachability declaration from pre-`permissions.network` manifests:
 * a top-level `network.hosts` array. Returns the host strings, or `[]`.
 */
function legacyNetworkHosts(manifest: any): string[] {
const hosts = manifest?.network?.hosts;
if (!Array.isArray(hosts)) return [];
return hosts.filter((h: unknown): h is string => typeof h === 'string' && h.length > 0);
}

/**
 * Derive the grants a manifest *declares*.
 *
 * When the manifest carries an explicit `permissions` block it is normalised
 * verbatim. Otherwise grants are inferred from structure (ui:// resources,
 * tool list, network hosts) and **identity is never inferred** — privacy by
 * default, matching `McpServer::get_permissions` on the Rust side.
 */
export function permissionsFromManifest(
manifest: any,
transport: string
): PluginPermissions {
if (manifest?.permissions) {
const norm = normalizePermissions(manifest.permissions);
if (transport === 'stdio' || transport === 'binary') {
norm.process = true;
}
// Precedence: `permissions.network.allow`, then the legacy top-level
// `network.hosts` array, then no network at all. An explicitly declared
// `network` block always wins — the legacy fallback only fills the gap.
const declaredNetwork = (manifest.permissions as any)?.network;
if (declaredNetwork == null) {
const legacy = legacyNetworkHosts(manifest);
if (legacy.length > 0) norm.network = { allow: legacy };
}
return norm;
}

const uiResources = manifestUiResources(manifest);
const hasUi = uiResources.length > 0 || Boolean(manifest?.ui_hooks?.length);
const hasTools = Array.isArray(manifest?.mcp?.tools) && manifest.mcp.tools.length > 0;
const legacyHosts = legacyNetworkHosts(manifest);
const isProcess = transport === 'stdio' || transport === 'binary';

return {
ui: {
	enabled: hasUi || isProcess,
	resources: hasUi ? uiResources : (isProcess ? ['*'] : [])
},
storage: { get: false, set: false, delete: false, keys: [] },
agent: { trigger: hasTools, tools: hasTools },
// Never inferred: the plugin must ask explicitly.
identity: {
'agent.name': false,
'agent.profile_picture': false,
'user.name': false,
'user.profile_picture': false
},
process: isProcess,
// An HTTP/SSE transport talks to an arbitrary endpoint and cannot be
// narrowed without a manifest allow-list, so it stays unrestricted.
network: { allow: legacyHosts.length > 0 ? legacyHosts : transport === 'http' ? ['*'] : [] },
// Never inferred. Only `permissions.elevated === true` sets this.
elevated: false,
// Never inferred either: control grants reach the server CRUD surface.
control: normalizeControl(null)
};
}

/** Parse `manifest_json` and derive its declared grants. */
export function permissionsForServer(server: McpServer): PluginPermissions {
let manifest: any = null;
if (server.manifest_json) {
try {
manifest = JSON.parse(server.manifest_json);
} catch {
manifest = null;
}
}
return permissionsFromManifest(manifest, server.transport);
}

/**
 * The grants actually in force: the plugin's *declared* grants narrowed by the
 * account's stored override (`permissions_json`), if any.
 */
export function resolvedPermissions(server: McpServer): PluginPermissions {
const declared = permissionsForServer(server);
if (!server.permissions_json) return declared;
try {
const stored = JSON.parse(server.permissions_json);
if (!isRecord(stored) || Object.keys(stored).length === 0) return declared;

// The canonical shape always has at least one *grouped section*, or an
// explicit `elevated` flag. An override saved in the removed flat format
// (`{ui: true, agent_control: true, ...}`) has neither; normalising it would
// silently deny every capability and hide the plugin's apps. Treat such a row
// as "no override" and fall back to the plugin's declared grants instead.
const isGrouped =
['ui', 'storage', 'agent', 'identity', 'control', 'network'].some((k) =>
isRecord((stored as any)[k])
) || typeof (stored as any).elevated === 'boolean';
if (!isGrouped) return declared;

const norm = normalizePermissions(stored);
if (server.transport === 'stdio' || server.transport === 'binary') {
norm.process = true;
}
// `elevated` is explicit consent: a manifest may declare it, and the operator
// may grant or revoke it in this editor. An older stored decision without an
// `elevated` key must keep the manifest's declaration rather than revoke it
// silently.
norm.elevated =
isRecord(stored) && typeof (stored as any).elevated === 'boolean'
? (stored as any).elevated === true
: declared.elevated;
return norm;
} catch {
return declared;
}
}

/** One row in a read-only "Required Access" summary. */
export interface RequiredAccessItem {
/** Group label, e.g. "Storage (isolated)". */
group: string;
/** Short scope name, e.g. "storage.get". */
scope: string;
/** Human explanation of what the scope allows. */
description: string;
}

/**
 * Flattens a permission set into the scopes it actually grants.
 *
 * Used to *display* what a plugin needs — it is informational, not a control.
 * Only granted scopes are returned, so an empty array means the plugin requests
 * no special access beyond being callable.
 */
export function requiredAccessList(perms: PluginPermissions): RequiredAccessItem[] {
const out: RequiredAccessItem[] = [];

if (perms.ui?.enabled) {
const n = perms.ui.resources?.length ?? 0;
out.push({
group: 'UI Hook',
scope: 'ui',
description: n > 0 ? `Renders ${n} sandboxed ui:// view(s)` : 'Renders a sandboxed UI view'
});
}

const st = perms.storage;
if (st?.get) out.push({ group: 'Storage (isolated)', scope: 'storage.get', description: 'Reads keys in its own namespace' });
if (st?.set) out.push({ group: 'Storage (isolated)', scope: 'storage.set', description: 'Writes keys in its own namespace' });
if (st?.delete) out.push({ group: 'Storage (isolated)', scope: 'storage.delete', description: 'Deletes keys in its own namespace' });

if (perms.agent?.trigger) out.push({ group: 'Agent', scope: 'agent.trigger', description: 'Starts an agent turn with its own text' });
if (perms.agent?.tools) out.push({ group: 'Agent', scope: 'agent.tools', description: 'Exposes its tools to the agent loop' });

const id = perms.identity;
if (id?.['agent.name']) out.push({ group: 'Profile & Identity', scope: 'identity.agent.name', description: 'Reads the agent display name' });
if (id?.['agent.profile_picture']) out.push({ group: 'Profile & Identity', scope: 'identity.agent.profile_picture', description: 'Reads the agent avatar' });
if (id?.['user.name']) out.push({ group: 'Profile & Identity', scope: 'identity.user.name', description: 'Reads your display name' });
if (id?.['user.profile_picture']) out.push({ group: 'Profile & Identity', scope: 'identity.user.profile_picture', description: 'Reads your avatar' });

if (perms.process) out.push({ group: 'Runtime', scope: 'process', description: 'Spawns a child process' });

const netAllow = perms.network?.allow ?? [];
if (netAllow.includes('*')) {
out.push({ group: 'Runtime', scope: 'network', description: 'Network (unrestricted): may connect to any host or IP' });
} else if (netAllow.length > 0) {
out.push({
group: 'Runtime',
scope: 'network',
description: `Network (allow-list): ${netAllow.join(', ')}`
});
}

// Only ever granted by an explicit manifest declaration — never inferred.
if (perms.elevated) {
out.push({
group: 'Runtime',
scope: 'elevated',
description: 'Elevated host access: runs on the host with no sandbox, no network or filesystem isolation'
});
}

const CONTROL_LABELS: Record<keyof ControlPermissions, string> = {
agents_read: 'reads any agent you can see',
agents_write: 'creates, edits and deletes agents',
shares: 'grants and revokes agent shares',
plugins_read: 'lists and reads plugins',
plugins_write: 'installs, edits and deletes plugins',
providers_read: 'lists and reads providers and models',
providers_write: 'creates, edits and deletes providers',
accounts: 'administers accounts (root credential only)'
};

if (perms.control) {
for (const key of Object.keys(CONTROL_LABELS) as Array<keyof ControlPermissions>) {
if (perms.control[key] !== true) continue;
out.push({
group: 'Server Control',
scope: `control.${key}`,
description: `Control MCP: ${CONTROL_LABELS[key]}`
});
}
}

return out;
}
