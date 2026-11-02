import type { McpServer, PluginPermissions } from '$lib/api';

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
network: false
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
out.network = input.network === true;
return out;
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
return normalizePermissions(manifest.permissions);
}

const uiResources = manifestUiResources(manifest);
const hasUi = uiResources.length > 0 || Boolean(manifest?.ui_hooks?.length);
const hasTools = Array.isArray(manifest?.mcp?.tools) && manifest.mcp.tools.length > 0;
const hasNetwork = Boolean(manifest?.network?.hosts?.length);
const storage = transport === 'wasm' || transport === 'wasm_stdio';

return {
ui: { enabled: hasUi, resources: uiResources },
storage: { get: storage, set: storage, delete: storage, keys: storage ? null : [] },
agent: { trigger: hasTools, tools: hasTools },
// Never inferred: the plugin must ask explicitly.
identity: {
'agent.name': false,
'agent.profile_picture': false,
'user.name': false,
'user.profile_picture': false
},
process: transport === 'stdio',
network: hasNetwork || transport === 'http'
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
return normalizePermissions(stored);
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
if (perms.network) out.push({ group: 'Runtime', scope: 'network', description: 'Makes outbound network requests' });

return out;
}
