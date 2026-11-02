export type AccountRole = 'root' | 'member';

export interface Profile {
	account_id: string;
	display_name?: string;
	avatar_color?: string;
	avatar_shape?: string;
	/** Data URL (uploaded picture) or absolute http(s) URL. */
	avatar_url?: string;
}

/** Payload accepted by PUT /api/profile */
export interface ProfileUpdatePayload {
	display_name?: string;
	avatar_color?: string;
	avatar_shape?: string;
	avatar_url?: string;
}

/** Response of GET /api/auth/me */
export interface AuthMe {
	account_id: string;
	username: string;
	role: AccountRole;
	profile?: Profile | null;
}

/** Response of GET /api/auth/login */
export interface AuthLoginResponse {
	token: string;
	account_id: string;
	username: string;
	role?: AccountRole;
}

/** Row returned by GET /api/accounts (root only). */
export interface ManagedAccount {
	id: string;
	username: string;
	role: AccountRole;
	display_name?: string;
	avatar_url?: string;
	created_at?: string;
}

export interface CreateAccountPayload {
	username: string;
	password: string;
	display_name?: string;
	role?: AccountRole;
}

export interface UpdateAccountPayload {
	username?: string;
	display_name?: string;
	role?: AccountRole;
	avatar_url?: string;
}

export interface ChangePasswordPayload {
	old_password: string;
	new_password: string;
	confirm_password: string;
}

export interface ModelConfigPayload {
	model_id: string;
	name?: string;
	config_json?: string;
}

export interface ProviderModel {
	id: string;
	provider_id: string;
	model_id: string;
	name?: string;
	config_json?: string;
	is_active: boolean;
	created_at?: string;
}

export interface Provider {
	id: string;
	account_id: string;
	name: string;
	base_url: string;
	api_key?: string;
	created_at?: string;
	models: ProviderModel[];
}

export interface ActiveModelTarget {
	id: string;
	provider_id: string;
	provider_name: string;
	base_url: string;
	api_key?: string;
	model_id: string;
	model_name?: string;
	config_json?: string;
}

export interface ResponseHook {
	id: string;
	message: string;
	source: string;
	enabled: boolean;
}

/** A reusable system instruction owned by one account. */
export interface InstructionPreset {
id: string;
account_id: string;
name: string;
description: string;
content: string;
created_at?: string;
updated_at?: string;
}

export interface InstructionPresetPayload {
name: string;
description?: string;
content: string;
}

export interface Agent {
	id: string;
	account_id: string;
	name: string;
	avatar_color?: string;
	avatar_shape?: string;
	avatar_url?: string;
	strategy: string;
	system_prompt?: string;
	is_active: boolean;
	allow_output_text?: boolean;
	output_text_error_message?: string;
	max_running_history_chars?: number;
/** Selected Agent Instructions preset; null/undefined = Custom. */
instruction_preset_id?: string | null;
	response_hooks_json?: string;
}

export interface AgentDetail {
	agent: Agent;
	provider_ids: string[];
	model_ids: string[];
	models?: ActiveModelTarget[];
	mcp_ids: string[];
}

/**
 * A dotted, namespaced grant. Only granted capabilities appear in the bridge
 * bootstrap; an absent capability means "not granted" (see docs/HOST_BRIDGE.md §1).
 */
export type Capability =
| 'ui.context.refresh'
| 'storage.get'
| 'storage.set'
| 'storage.delete'
| 'storage.keys'
| 'agent.trigger'
| 'identity.agent.name'
| 'identity.agent.profile_picture'
| 'identity.user.name'
| 'identity.user.profile_picture';

/**
 * Canonical permission object (docs/HOST_BRIDGE.md §2/§6).
 * This is the only accepted shape — there is no flat fallback.
 */
export interface PluginPermissions {
/** `resources` is an allow-list of `ui://` URIs; `["*"]` means "every resource". */
ui: { enabled: boolean; resources: string[] };
/** `keys` is a list of key prefixes; `null` means "all keys". */
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

/** Avatar descriptor as returned by the host. Never assembled browser-side. */
export interface HostBridgeAvatar {
url?: string;
color?: string;
shape?: string;
}

/** Identity block of the bridge bootstrap (§6). Absent = not granted (§3 omission rule). */
export interface HostBridgeIdentity {
agent?: { id: string; name?: string; avatar?: HostBridgeAvatar };
user?: { name?: string; avatar?: HostBridgeAvatar };
}

/** Payload returned by `GET /api/host/session` and pushed on `ui/context` (§3). */
export interface HostBridgeBootstrap {
protocolVersion: string;
app: { pluginId: string; pluginName: string; resourceUri: string; title: string };
agent?: HostBridgeIdentity['agent'];
user?: HostBridgeIdentity['user'];
capabilities: Capability[];
permissions: PluginPermissions;
context: { locale: string | null; theme: 'light' | 'dark' };
}

/** `storage.get` value. */
export interface HostStorageValue {
key: string;
value: string | null;
}

/** `storage.set` / `storage.delete` acknowledgement. */
export interface HostStorageMutation {
ok: boolean;
key?: string;
}

/** `storage.keys` / `storage.list` result. */
export interface HostStorageKeyList {
keys: string[];
}

export interface CustomToolDef {
	name: string;
	description?: string;
	method: string;
	url: string;
	headers?: Record<string, string>;
	body?: string;
	cookie_expression?: string;
	captures?: {
		cookies?: string;
		json_fields?: Record<string, string>;
	};
	input_schema?: any;
}

export interface TestCustomToolResult {
	success: boolean;
	status: number;
	body: string;
	updated_session_variables: Record<string, string>;
	error?: string;
}

export interface McpServer {
	id: string;
	account_id: string;
	name: string;
	alias?: string;
	transport: string;
	url_or_command: string;
	headers_or_env?: string;
	version?: string;
	description?: string;
	wasm_hash?: string;
	manifest_json?: string;
	permissions_json?: string;
	is_active?: boolean;
	storage_bytes?: number;
	created_at?: string;
}

export interface AgentPushedMessage {
	id: string;
	agent_id: string;
	mcp_id?: string;
	method: string;
	params_json?: string;
	created_at?: string;
}

export interface AgentToolItem {
	mcp_id: string;
	server_name: string;
	server_alias?: string;
	name: string;
	unique_name?: string;
	description?: string;
	input_schema?: any;
	meta?: any;
	is_enabled: boolean;
}

export interface AgentResourceItem {
	mcp_id: string;
	server_name: string;
	uri: string;
	name: string;
	description?: string;
	mime_type?: string;
	is_subscribed: boolean;
}

export interface AgentToolsResponse {
	tools: AgentToolItem[];
	resources: AgentResourceItem[];
	subscriptions: string[];
}

export interface AgentHistoryItem {
	id: string;
	agent_id: string;
	role: 'system' | 'user' | 'assistant' | 'tool';
	content: string;
	tool_calls_json?: string;
	tool_call_id?: string;
	created_at?: string;
}

export interface ServerAccount {
	id: string;
	serverUrl: string;
	username: string;
	token: string;
	role?: AccountRole;
	displayName?: string;
	avatarColor?: string;
	avatarShape?: string;
	avatarUrl?: string;
}

export interface ExtensionManifest {
	id: string;
	name: string;
	version: string;
	description?: string;
	/** Resolved grants declared by this plugin (see docs/HOST_BRIDGE.md). */
	permissions?: Partial<PluginPermissions>;
	mcp?: {
		tools?: any[];
		resources?: {
			uri: string;
			name: string;
			description?: string;
			mimeType?: string;
			mime_type?: string;
		}[];
	};
	data_access?: string[];
	network?: { hosts?: string[] };
}

export interface ExtensionItem {
	id: string;
	name: string;
	version: string;
	description?: string;
	wasm_hash: string;
	manifest: ExtensionManifest;
	is_active: boolean;
	storage_bytes: number;
	created_at?: string;
}

export interface ExtensionKvEntry {
	extension_id: string;
	key: string;
	value: string;
	updated_at?: string;
}

export interface ExtensionStorageInfo {
	extension_id: string;
	wasm_hash: string;
	total_storage_bytes: number;
	files: { name: string; size_bytes: number }[];
	kv_count: number;
}

class ApiClient {
	private serverUrl = 'http://localhost:8000';
	private token = '';

	constructor() {
		if (typeof window !== 'undefined') {
			this.loadActiveProfile();
		}
	}

	public setConfig(serverUrl: string, token: string) {
		this.serverUrl = serverUrl.replace(/\/+$/, '');
		this.token = token;
	}

	public getServerUrl() {
		return this.serverUrl;
	}

	public getToken() {
		return this.token;
	}

	public loadActiveProfile() {
		const saved = localStorage.getItem('neuxdot_active_account');
		if (saved) {
			try {
				const acc = JSON.parse(saved) as ServerAccount;
				this.serverUrl = acc.serverUrl || 'http://localhost:8000';
				this.token = acc.token || '';
			} catch {}
		}
	}

	private async request<T>(endpoint: string, options: RequestInit = {}): Promise<T> {
		const url = `${this.serverUrl}${endpoint}`;
		const headers = new Headers(options.headers || {});
		headers.set('Content-Type', 'application/json');
		if (this.token) {
			headers.set('Authorization', `Bearer ${this.token}`);
		}

		const res = await fetch(url, {
			...options,
			headers
		});

		if (!res.ok) {
			const errorData = await res.json().catch(() => ({ error: res.statusText }));
			throw new Error(errorData.error || `Request failed with status ${res.status}`);
		}

		// Several endpoints (DELETE /api/accounts/:id and friends) answer with
		// 204 No Content, which has an empty body. Calling res.json() on those
		// throws "Unexpected end of JSON input" and makes a successful request
		// look like a failure.
		if (res.status === 204 || res.headers.get('content-length') === '0') {
			return undefined as T;
		}

		const text = await res.text();
		if (!text) {
			return undefined as T;
		}
		return JSON.parse(text) as T;
	}

	// Auth
	auth = {
		login: (username: string, password: string) =>
			this.request<AuthLoginResponse>('/api/auth/login', {
				method: 'POST',
				body: JSON.stringify({ username, password })
			}),

		me: () => this.request<AuthMe>('/api/auth/me')
	};

	// Accounts (root only; non-root receives 403)
	accounts = {
		list: () => this.request<ManagedAccount[]>('/api/accounts'),

		create: (data: CreateAccountPayload) =>
			this.request<ManagedAccount>('/api/accounts', {
				method: 'POST',
				body: JSON.stringify(data)
			}),

		update: (id: string, data: UpdateAccountPayload) =>
			this.request<ManagedAccount>(`/api/accounts/${id}`, {
				method: 'PATCH',
				body: JSON.stringify(data)
			}),

		delete: (id: string) =>
			this.request<{ deleted: boolean }>(`/api/accounts/${id}`, { method: 'DELETE' }),

		/** Change the password of the *currently authenticated* account (401 = wrong old password, 400 = mismatch). */
		changePassword: (id: string, data: ChangePasswordPayload) =>
			this.request<{ success: boolean }>(`/api/accounts/${id}/password`, {
				method: 'POST',
				body: JSON.stringify(data)
			})
	};

	// Agents
	agents = {
		list: () => this.request<Agent[]>('/api/agents'),

		get: (id: string) => this.request<AgentDetail>(`/api/agents/${id}`),

		create: (data: {
			name: string;
			avatar_color?: string;
			avatar_shape?: string;
			avatar_url?: string;
			strategy?: string;
			system_prompt?: string;
			provider_ids?: string[];
			model_ids?: string[];
			mcp_ids?: string[];
			allow_output_text?: boolean;
			output_text_error_message?: string;
			max_running_history_chars?: number;
			response_hooks_json?: string;
			response_hooks?: ResponseHook[];
			/**
			 * Selected Agent Instructions preset. Use `''` for Custom.
			 */
			instruction_preset_id?: string | null;
		}) =>
			this.request<Agent>('/api/agents', {
				method: 'POST',
				body: JSON.stringify(data)
			}),

		patch: (
			id: string,
			data: {
				name?: string;
				avatar_color?: string;
				avatar_shape?: string;
				avatar_url?: string;
				strategy?: string;
				system_prompt?: string;
				is_active?: boolean;
				provider_ids?: string[];
				model_ids?: string[];
				mcp_ids?: string[];
				allow_output_text?: boolean;
				output_text_error_message?: string;
				max_running_history_chars?: number;
				response_hooks_json?: string;
				response_hooks?: ResponseHook[];
				/**
				 * Selected Agent Instructions preset. Send `''` to detach and go
				 * back to Custom; omit to leave the selection untouched.
				 */
				instruction_preset_id?: string | null;
			}
		) =>
			this.request<Agent>(`/api/agents/${id}`, {
				method: 'PATCH',
				body: JSON.stringify(data)
			}),

		delete: (id: string) =>
			this.request<{ deleted: boolean }>(`/api/agents/${id}`, { method: 'DELETE' }),

		run: (id: string, input: string) =>
			this.request<{ status: string }>(`/api/agents/${id}/run`, {
				method: 'POST',
				body: JSON.stringify({ input })
			}),

		pushedMessages: (id: string) =>
			this.request<AgentPushedMessage[]>(`/api/agents/${id}/pushed`),

		getMemory: (id: string) =>
			this.request<{
				agent_id: string;
				content: string;
				current_hash: string;
				character_count: number;
				updated_at?: string;
			}>(`/api/agents/${id}/memory`),

		getHistory: (id: string) =>
			this.request<AgentHistoryItem[]>(`/api/agents/${id}/history`),

		clearHistory: (id: string) =>
			this.request<{ cleared: boolean }>(`/api/agents/${id}/history`, {
				method: 'DELETE'
			}),

		getTools: (id: string) => this.request<AgentToolsResponse>(`/api/agents/${id}/tools`),

		setToolPermission: (id: string, mcp_id: string, tool_name: string, is_enabled: boolean) =>
			this.request<{ success: boolean }>(`/api/agents/${id}/tools`, {
				method: 'PUT',
				body: JSON.stringify({ mcp_id, tool_name, is_enabled })
			}),

		getSubscriptions: (id: string) => this.request<string[]>(`/api/agents/${id}/subscriptions`),

		setSubscriptions: (id: string, subscriptions: string[]) =>
			this.request<{ success: boolean; subscriptions: string[] }>(`/api/agents/${id}/subscriptions`, {
				method: 'PUT',
				body: JSON.stringify({ subscriptions })
			})
	};

	// Providers
	providers = {
		list: () => this.request<Provider[]>('/api/providers'),

		create: (data: {
			name: string;
			base_url: string;
			api_key?: string;
			models?: (string | ModelConfigPayload)[];
		}) =>
			this.request<Provider>('/api/providers', {
				method: 'POST',
				body: JSON.stringify(data)
			}),

		update: (
			id: string,
			data: {
				name: string;
				base_url: string;
				api_key?: string;
				models?: (string | ModelConfigPayload)[];
			}
		) =>
			this.request<Provider>(`/api/providers/${id}`, {
				method: 'PUT',
				body: JSON.stringify(data)
			}),

		delete: (id: string) =>
			this.request<{ deleted: boolean }>(`/api/providers/${id}`, { method: 'DELETE' }),

		fetchModels: (baseUrl: string, apiKey?: string) =>
			this.request<{ models: string[] }>('/api/providers/fetch-models', {
				method: 'POST',
				body: JSON.stringify({ base_url: baseUrl, api_key: apiKey || undefined })
			})
	};

	// Plugins & MCP Servers
	mcp = {
		list: () => this.request<McpServer[]>('/api/mcp'),

		create: (data: {
			name: string;
			alias?: string;
			transport: string;
			url_or_command: string;
			headers_or_env?: string;
			is_active?: boolean;
			permissions?: PluginPermissions;
		}) =>
			this.request<McpServer>('/api/mcp', {
				method: 'POST',
				body: JSON.stringify(data)
			}),

		update: (
			id: string,
			data: {
				name?: string;
				alias?: string;
				transport?: string;
				url_or_command?: string;
				headers_or_env?: string;
				is_active?: boolean;
				permissions?: PluginPermissions;
			}
		) =>
			this.request<McpServer>(`/api/mcp/${encodeURIComponent(id)}`, {
				method: 'PUT',
				body: JSON.stringify(data)
			}),

		delete: (id: string) =>
			this.request<{ deleted: boolean }>(`/api/mcp/${encodeURIComponent(id)}`, {
				method: 'DELETE'
			}),

		push: (id: string, method: string, params?: unknown, target_agent_id?: string) =>
			this.request<{ status: string; affected_agents_count: number }>(
				`/api/mcp/${encodeURIComponent(id)}/push`,
				{
					method: 'POST',
					body: JSON.stringify({ method, params, target_agent_id })
				}
			),

		rpc: <T = any>(id: string, method: string, params?: unknown, agent_id?: string) =>
			this.request<T>(`/api/plugins/${encodeURIComponent(id)}/rpc`, {
				method: 'POST',
				body: JSON.stringify({ method, params, agent_id })
			}),

		testCustomTool: (payload: {
			tool: CustomToolDef;
			arguments?: any;
			session_variables?: Record<string, string>;
		}) =>
			this.request<TestCustomToolResult>('/api/mcp/test-custom-tool', {
				method: 'POST',
				body: JSON.stringify(payload)
			})
	};

	// Host bridge (sandboxed MCP-App <-> host). See docs/HOST_BRIDGE.md §5.
host = {
/**
 * Builds the ui/context bootstrap server-side from the session, the agent
 * row and the profile row. 403 when the plugin/agent belongs to another
 * account. This is the only sanctioned way for a sandboxed app to learn
 * identity — the browser never reads the profile itself.
 */
session: (pluginId: string, agentId: string, resourceUri: string) =>
this.request<HostBridgeBootstrap>(
`/api/host/session?plugin_id=${encodeURIComponent(pluginId)}` +
`&agent_id=${encodeURIComponent(agentId)}` +
`&resource_uri=${encodeURIComponent(resourceUri)}`
),

/** Per-(account, plugin) isolated key/value storage, gated by `storage.*` grants. */
storage: {
get: (pluginId: string, key: string) =>
this.request<HostStorageValue>('/api/host/storage/get', {
method: 'POST',
body: JSON.stringify({ plugin_id: pluginId, key })
}),

set: (pluginId: string, key: string, value: string) =>
this.request<HostStorageMutation>('/api/host/storage/set', {
method: 'POST',
body: JSON.stringify({ plugin_id: pluginId, key, value })
}),

delete: (pluginId: string, key: string) =>
this.request<HostStorageMutation>('/api/host/storage/delete', {
method: 'POST',
body: JSON.stringify({ plugin_id: pluginId, key })
}),

list: (pluginId: string, prefix?: string) =>
this.request<HostStorageKeyList>('/api/host/storage/list', {
method: 'POST',
body: JSON.stringify({ plugin_id: pluginId, prefix })
})
}
};

	// Profile
	profile = {
		get: () => this.request<Profile>('/api/profile'),

		update: (data: ProfileUpdatePayload) =>
			this.request<Profile>('/api/profile', {
				method: 'PUT',
				body: JSON.stringify(data)
			})
	};

// Agent Instructions presets (owner-scoped to the signed-in account)
instructionPresets = {
list: () => this.request<InstructionPreset[]>('/api/instruction-presets'),

create: (data: InstructionPresetPayload) =>
this.request<InstructionPreset>('/api/instruction-presets', {
method: 'POST',
body: JSON.stringify(data)
}),

update: (id: string, data: Partial<InstructionPresetPayload>) =>
this.request<InstructionPreset>(`/api/instruction-presets/${id}`, {
method: 'PATCH',
body: JSON.stringify(data)
}),

delete: (id: string) =>
this.request<void>(`/api/instruction-presets/${id}`, { method: 'DELETE' })
};

	// Extensions & WASM Modules
	extensions = {
		list: () => this.request<ExtensionItem[]>('/api/extensions'),

		inspect: async (file: File) => {
			const formData = new FormData();
			formData.append('file', file);
			const url = `${this.serverUrl}/api/extensions/inspect`;
			const res = await fetch(url, {
				method: 'POST',
				headers: this.token ? { Authorization: `Bearer ${this.token}` } : {},
				body: formData
			});
			if (!res.ok) {
				const err = await res.text();
				throw new Error(err || 'Failed to inspect extension WASM');
			}
			return (await res.json()) as {
				id: string;
				name: string;
				version: string;
				description?: string;
				wasm_hash: string;
				manifest: any;
				existing: ExtensionItem | null;
			};
		},

		upload: async (file: File) => {
			const formData = new FormData();
			formData.append('file', file);
			const url = `${this.serverUrl}/api/extensions/upload`;
			const res = await fetch(url, {
				method: 'POST',
				headers: this.token ? { Authorization: `Bearer ${this.token}` } : {},
				body: formData
			});
			if (!res.ok) {
				const err = await res.text();
				throw new Error(err || 'Failed to upload extension WASM');
			}
			return (await res.json()) as ExtensionItem;
		},

		installUrl: (url: string) =>
			this.request<ExtensionItem>('/api/extensions/url', {
				method: 'POST',
				body: JSON.stringify({ url })
			}),

		patch: (id: string, is_active: boolean) =>
			this.request<{ success: boolean }>(`/api/extensions/${id}`, {
				method: 'PATCH',
				body: JSON.stringify({ is_active })
			}),

		delete: (id: string) =>
			this.request<{ success: boolean }>(`/api/extensions/${id}`, { method: 'DELETE' }),

		getKv: (id: string) => this.request<ExtensionKvEntry[]>(`/api/extensions/${id}/kv`),

		deleteKvKey: (id: string, key: string) =>
			this.request<{ success: boolean }>(`/api/extensions/${id}/kv/${encodeURIComponent(key)}`, {
				method: 'DELETE'
			}),

		getStorage: (id: string) =>
			this.request<ExtensionStorageInfo>(`/api/extensions/${encodeURIComponent(id)}/storage`)
	};

	// Unified Plugins alias combining MCP + WASM
	plugins = {
		list: () => this.mcp.list(),
		create: (data: Parameters<typeof this.mcp.create>[0]) => this.mcp.create(data),
		update: (id: string, data: Parameters<typeof this.mcp.update>[1]) => this.mcp.update(id, data),
		delete: (id: string) => this.mcp.delete(id),
		push: (id: string, method: string, params?: unknown, target_agent_id?: string) =>
			this.mcp.push(id, method, params, target_agent_id),
		rpc: <T = any>(id: string, method: string, params?: unknown, agent_id?: string) =>
			this.mcp.rpc<T>(id, method, params, agent_id),
		testCustomTool: (payload: {
			tool: CustomToolDef;
			arguments?: any;
			session_variables?: Record<string, string>;
		}) => this.mcp.testCustomTool(payload),
		inspectWasm: (file: File) => this.extensions.inspect(file),
		uploadWasm: (file: File) => this.extensions.upload(file),
		installWasmUrl: (url: string) => this.extensions.installUrl(url),
		getKv: (id: string) => this.extensions.getKv(id),
		deleteKvKey: (id: string, key: string) => this.extensions.deleteKvKey(id, key),
		getStorage: (id: string) => this.extensions.getStorage(id)
	};
}

export const ROLE_ROOT: AccountRole = 'root';
export const ROLE_MEMBER: AccountRole = 'member';

/** True when the given account/role holds root (administrative) privileges. */
export function isRoot(account?: { role?: AccountRole | null } | AccountRole | null): boolean {
	if (!account) return false;
	if (typeof account === 'string') return account === ROLE_ROOT;
	return account.role === ROLE_ROOT;
}

export const api = new ApiClient();
