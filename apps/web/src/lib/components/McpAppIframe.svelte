<script lang="ts">
import { onMount, onDestroy } from 'svelte';
import {
api,
type Agent,
type Capability,
type HostBridgeBootstrap,
type PluginPermissions
} from '$lib/api';
import { authState } from '$lib/stores/auth.svelte';
import { RefreshCw } from 'lucide-svelte';

let {
agent,
pluginId,
pluginName = '',
transport = 'wasm_stdio',
resourceUri,
title = 'MCP-App',
permissions
}: {
agent: Agent;
pluginId: string;
pluginName?: string;
transport?: string;
resourceUri: string;
title?: string;
/**
 * Grants resolved by the caller (the /mcp page). Only used to seed the
 * local fallback bootstrap — the server remains the authority on consent.
 */
permissions?: PluginPermissions;
} = $props();

let iframeEl = $state<HTMLIFrameElement | null>(null);
let htmlContent = $state('');
let loading = $state(true);
let error = $state('');
let lastKey = '';
let rpcCallCount = $state(0);
/** Bootstrap resolved by the server, pushed to the app on load and on `ui/context`. */
let bootstrap = $state<HostBridgeBootstrap | null>(null);

$effect(() => {
const key = `${pluginId}:${resourceUri}`;
if (key !== lastKey) {
lastKey = key;
loadMcpAppResource();
}
});

async function loadMcpAppResource() {
loading = true;
error = '';
try {
// Resolve consent + identity server-side first, so the app can be
// handed real values instead of fetching anything itself.
bootstrap = await resolveBootstrap();

const res = await api.plugins.rpc(pluginId, 'resources/read', { uri: resourceUri }, agent.id);
const contents = res?.contents || [];
const item = contents[0];
if (item?.text) {
htmlContent = item.text;
} else {
error = `MCP-App resource ${resourceUri} returned no HTML text content.`;
}
} catch (e: any) {
error = e.message || `Failed to load MCP-App resource ${resourceUri}`;
} finally {
loading = false;
}
}

/**
 * Ask the server to build the bridge bootstrap (`GET /api/host/session`).
 * This is the API-proxy path: the browser never reads the profile or the
 * agent row to assemble identity itself.
 */
async function resolveBootstrap(): Promise<HostBridgeBootstrap> {
try {
return await api.host.session(pluginId, agent.id, resourceUri);
} catch (e) {
console.warn('[McpAppIframe] /api/host/session unavailable, using local fallback', e);
return buildLocalBootstrap();
}
}

/**
 * FALLBACK ONLY — used when the host session endpoint is unreachable.
 * Deliberately conservative: it grants nothing and ships no identity, so a
 * transient backend failure can never leak profile data into the sandbox.
 */
function buildLocalBootstrap(): HostBridgeBootstrap {
return {
protocolVersion: '2025-03-26',
app: { pluginId, pluginName, resourceUri, title },
capabilities: [],
permissions:
permissions ??
({
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
} satisfies PluginPermissions),
context: { locale: null, theme: 'light' }
};
}

/**
 * Structural-clone-safe postMessage.
 *
 * Svelte 5 `$state` values are Proxies and cannot be structured-cloned, which
 * is what used to throw `DOMException: Proxy object could not be cloned`.
 * Everything leaving this component goes through `$state.snapshot` and is
 * serialised to a plain JSON string as a belt-and-braces guarantee that no
 * proxy, getter or class instance survives the trip.
 */
function postToApp(payload: Record<string, unknown>) {
const win = iframeEl?.contentWindow;
if (!win) return;
try {
const plain = JSON.parse(JSON.stringify($state.snapshot(payload)));
win.postMessage(plain, '*');
} catch (e) {
console.error('[McpAppIframe] failed to postMessage to app', e);
}
}

function pushContext() {
if (!bootstrap) return;
postToApp({ jsonrpc: '2.0', method: 'ui/context', params: $state.snapshot(bootstrap) });
}

function sendContextToIframe() {
// The iframe reloaded: hand it the current bootstrap straight away so a
// plugin that never calls ui/initialize still renders.
pushContext();
}

function granted(cap: Capability): boolean {
return bootstrap?.capabilities.includes(cap) ?? false;
}

function rpcError(message: string, code = -32000) {
return { code, message };
}

/**
 * Single dispatcher for `host/call`. Mirrors docs/HOST_BRIDGE.md §4.
 * The server is authoritative; the grant checks here are defence in depth so
 * a bug in one surface cannot alone hand out data.
 */
async function handleHostCall(capability: Capability, args: any) {
if (!granted(capability)) {
throw rpcError(`Capability '${capability}' is not granted to this plugin`, -32004);
}

switch (capability) {
case 'ui.context.refresh': {
bootstrap = await resolveBootstrap();
return { ok: true, value: $state.snapshot(bootstrap) };
}
case 'identity.agent.name':
return { ok: true, value: bootstrap?.agent?.name ?? null };
case 'identity.agent.profile_picture':
return { ok: true, value: bootstrap?.agent?.avatar ?? null };
case 'identity.user.name':
return { ok: true, value: bootstrap?.user?.name ?? null };
case 'identity.user.profile_picture':
return { ok: true, value: bootstrap?.user?.avatar ?? null };
case 'agent.trigger': {
const text = typeof args?.text === 'string' ? args.text : '';
if (!text.trim()) throw rpcError('agent.trigger requires a non-empty `text`');
const res = await api.agents.run(agent.id, text);
return { ok: true, value: res ?? null };
}
case 'storage.get': {
const res = await api.host.storage.get(pluginId, String(args?.key ?? ''));
return { ok: true, value: res?.value ?? null };
}
case 'storage.set': {
const res = await api.host.storage.set(
pluginId,
String(args?.key ?? ''),
typeof args?.value === 'string' ? args.value : JSON.stringify(args?.value ?? null)
);
return { ok: true, value: res ?? null };
}
case 'storage.delete': {
const res = await api.host.storage.delete(pluginId, String(args?.key ?? ''));
return { ok: true, value: res ?? null };
}
case 'storage.keys': {
const res = await api.host.storage.list(pluginId, args?.prefix ?? undefined);
return { ok: true, value: res?.keys ?? [] };
}
default:
throw rpcError(`Unsupported capability '${capability}'`);
}
}

async function handleMessage(event: MessageEvent) {
// The sandbox has an opaque origin, so `event.origin` is the string
// "null" and cannot authenticate the sender. `event.source` identity is
// the real boundary. (Never add `allow-same-origin`: that would give the
// app the host origin's storage and cookies.)
if (!iframeEl || event.source !== iframeEl.contentWindow) return;
const data = event.data;
if (!data || data.jsonrpc !== '2.0' || !data.method) return;

// Notification from iframe (no id)
if (data.id === undefined || data.id === null) {
if (data.method === 'ui/message' && data.params?.text) {
if (granted('agent.trigger')) {
api.agents.run(agent.id, data.params.text).catch(() => {});
}
}
return;
}

rpcCallCount += 1;

const reply = (result: unknown) =>
postToApp({ jsonrpc: '2.0', id: data.id, result });
const fail = (err: { code: number; message: string }) =>
postToApp({ jsonrpc: '2.0', id: data.id, error: err });

try {
if (data.method === 'ui/initialize') {
reply($state.snapshot(bootstrap ?? (await resolveBootstrap())));
} else if (data.method === 'host/call') {
const capability = data.params?.capability as Capability;
reply(await handleHostCall(capability, data.params?.args));
} else if (
data.method === 'tools/call' ||
data.method === 'resources/read' ||
data.method === 'resources/list' ||
data.method === 'resources/subscribe'
) {
reply(await api.plugins.rpc(pluginId, data.method, data.params || {}, agent.id));
} else if (data.method === 'ui/message') {
if (!granted('agent.trigger')) {
throw rpcError('Plugin does not have the agent.trigger capability', -32004);
}
reply(await api.agents.run(agent.id, data.params?.text || ''));
} else {
throw rpcError(`Unsupported MCP-App JSON-RPC method: ${data.method}`);
}
} catch (err: any) {
// Both our own rpcError objects and thrown Error/JSON-RPC payloads land here.
const code = typeof err?.code === 'number' ? err.code : -32000;
fail({ code, message: err?.message || String(err) });
}
}

onMount(() => {
window.addEventListener('message', handleMessage);
});

onDestroy(() => {
if (typeof window !== 'undefined') {
window.removeEventListener('message', handleMessage);
}
});
</script>

<div class="flex flex-1 flex-col h-full overflow-hidden bg-white dark:bg-zinc-950">

	{#if loading}
		<div class="flex flex-1 flex-col items-center justify-center p-8 text-zinc-400 dark:text-zinc-500 gap-2.5">
			<RefreshCw class="h-5 w-5 animate-spin text-zinc-400" />
			<span class="text-xs font-medium">Loading {title}...</span>
		</div>
	{:else if error}
		<div class="m-auto max-w-md rounded-2xl border border-rose-500/20 bg-rose-500/5 dark:bg-rose-500/10 p-5 text-xs text-rose-600 dark:text-rose-400 space-y-2">
			<div class="font-semibold text-rose-700 dark:text-rose-300 text-sm">Failed to Load Interface</div>
			<p class="leading-relaxed">{error}</p>
			<div class="pt-2 flex items-center gap-3">
				<button
					type="button"
					onclick={loadMcpAppResource}
					class="rounded-xl bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 px-3 py-1.5 text-xs font-medium hover:opacity-90 transition-opacity cursor-pointer"
				>
					Retry
				</button>
				<a href="/mcp" class="text-xs text-zinc-600 dark:text-zinc-400 underline hover:text-zinc-900 dark:hover:text-white">
					Configure Plugins
				</a>
			</div>
		</div>
	{:else}
		<iframe
			bind:this={iframeEl}
			title={title}
			sandbox="allow-scripts"
			srcdoc={htmlContent}
			onload={sendContextToIframe}
			class="h-full w-full flex-1 border-0 bg-transparent"
		></iframe>
	{/if}
</div>
