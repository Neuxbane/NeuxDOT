export interface Profile {
	account_id: string;
	display_name?: string;
	avatar_color?: string;
	avatar_shape?: string;
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

export interface Agent {
	id: string;
	account_id: string;
	name: string;
	avatar_color?: string;
	avatar_shape?: string;
	strategy: string;
	system_prompt?: string;
	is_active: boolean;
	allow_output_text?: boolean;
	output_text_error_message?: string;
	max_running_history_chars?: number;
	response_hooks_json?: string;
}

export interface AgentDetail {
	agent: Agent;
	provider_ids: string[];
	model_ids: string[];
	models?: ActiveModelTarget[];
	mcp_ids: string[];
}

export interface McpServer {
	id: string;
	account_id: string;
	name: string;
	alias?: string;
	transport: string;
	url_or_command: string;
	headers_or_env?: string;
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
	displayName?: string;
	avatarColor?: string;
}

export interface ExtensionUiHook {
	target_id: string;
	type?: string;
	panel_id?: string;
	title: string;
	icon?: string;
	template?: string;
}

export interface ExtensionManifest {
	id: string;
	name: string;
	version: string;
	description?: string;
	ui_hooks?: ExtensionUiHook[];
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

		return res.json();
	}

	// Auth
	auth = {
		login: (username: string, password: string) =>
			this.request<{ token: string; account_id: string; username: string }>('/api/auth/login', {
				method: 'POST',
				body: JSON.stringify({ username, password })
			}),

		me: () =>
			this.request<{ account_id: string; username: string; profile: Profile | null }>('/api/auth/me')
	};

	// Agents
	agents = {
		list: () => this.request<Agent[]>('/api/agents'),

		get: (id: string) => this.request<AgentDetail>(`/api/agents/${id}`),

		create: (data: {
			name: string;
			avatar_color?: string;
			avatar_shape?: string;
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

	// MCP Servers
	mcp = {
		list: () => this.request<McpServer[]>('/api/mcp'),

		create: (data: { name: string; alias?: string; transport: string; url_or_command: string; headers_or_env?: string }) =>
			this.request<McpServer>('/api/mcp', {
				method: 'POST',
				body: JSON.stringify(data)
			}),

		update: (id: string, data: { name: string; alias?: string; transport: string; url_or_command: string; headers_or_env?: string }) =>
			this.request<McpServer>(`/api/mcp/${id}`, {
				method: 'PUT',
				body: JSON.stringify(data)
			}),

		delete: (id: string) =>
			this.request<{ deleted: boolean }>(`/api/mcp/${id}`, { method: 'DELETE' }),

		push: (id: string, method: string, params?: unknown, target_agent_id?: string) =>
			this.request<{ status: string; affected_agents_count: number }>(`/api/mcp/${id}/push`, {
				method: 'POST',
				body: JSON.stringify({ method, params, target_agent_id })
			})
	};

	// Profile
	profile = {
		get: () => this.request<Profile>('/api/profile'),

		update: (data: { display_name?: string; avatar_color?: string; avatar_shape?: string }) =>
			this.request<Profile>('/api/profile', {
				method: 'PUT',
				body: JSON.stringify(data)
			})
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
			this.request<ExtensionStorageInfo>(`/api/extensions/${id}/storage`),

		executeAction: <T = unknown>(id: string, action: string, payload: unknown) =>
			this.request<T>(`/api/extensions/${id}/action`, {
				method: 'POST',
				body: JSON.stringify({ action, payload })
			})
	};
}

export const api = new ApiClient();
