<script lang="ts">
	import {
		api,
		type McpServer,
		type AgentToolItem,
		type AgentResourceItem
	} from '$lib/api';
	import {
		Server,
		Radio,
		Terminal,
		Sliders,
		Lock,
		CheckCircle2,
		RadioTower,
		Zap,
		ChevronDown,
		ChevronUp,
		Cpu,
		EyeOff,
		ShieldCheck,
		RefreshCw,
		Key,
		X,
		Globe,
		Bot
	} from 'lucide-svelte';

	interface Props {
		agentId?: string;
		allServers: McpServer[];
		selectedIds: string[];
		onchange?: (selected: string[]) => void;
	}

	let { agentId = '', allServers = [], selectedIds = $bindable([]), onchange }: Props = $props();

/**
 * A `ui://` resource is an MCP-App *view* (HTML interface served to the
 * sandboxed iframe), not model context. It must never be offered as a push
 * subscription: that would stream the whole document into the prompt.
 */
function isUiResource(r: AgentResourceItem): boolean {
return r.uri?.startsWith('ui://') || (r.mime_type || '').includes('mcp-app');
}

	let agentTools = $state<AgentToolItem[]>([]);
	let agentResources = $state<AgentResourceItem[]>([]);
	let subscriptions = $state<string[]>([]);
	let expandedServerId = $state<string | null>(null);
	let isLoadingTools = $state(false);

	let configServer = $state<McpServer | null>(null);
	let configModalOpen = $state(false);
	let isSavingConfig = $state(false);
	let overrideEnabled = $state(false);
	let configOverrideRaw = $state('');
	let globalConfigRaw = $state<string | null>(null);
	let agentConfigs = $state<Record<string, boolean>>({});

	$effect(() => {
		if (agentId) {
			loadAgentConfigs();
		}
	});

	async function loadAgentConfigs() {
		if (!agentId) return;
		try {
			const res = await api.agents.get(agentId);
			if (res.mcp_configs) {
				const map: Record<string, boolean> = {};
				for (const [k, v] of Object.entries(res.mcp_configs)) {
					if (v && v.trim().length > 0) {
						map[k] = true;
					}
				}
				agentConfigs = map;
			}
		} catch (e) {
			console.error('Failed to load agent MCP configs:', e);
		}
	}

	async function openConfigModal(srv: McpServer) {
		if (!agentId) return;
		configServer = srv;
		configModalOpen = true;
		try {
			const res = await api.agents.getMcpConfig(agentId, srv.id);
			globalConfigRaw = res.global_config || srv.headers_or_env || null;
			configOverrideRaw = res.config_override || '';
			overrideEnabled = res.has_override;
		} catch (e) {
			console.error('Failed to load plugin config:', e);
			globalConfigRaw = srv.headers_or_env || null;
			configOverrideRaw = '';
			overrideEnabled = false;
		}
	}

	function closeConfigModal() {
		configModalOpen = false;
		configServer = null;
	}

	async function saveConfigOverride() {
		if (!agentId || !configServer) return;
		isSavingConfig = true;
		try {
			const payload = overrideEnabled ? configOverrideRaw.trim() || null : null;
			await api.agents.setMcpConfig(agentId, configServer.id, payload);
			agentConfigs[configServer.id] = !!payload;
			closeConfigModal();
		} catch (e) {
			alert('Failed to save configuration: ' + e);
		} finally {
			isSavingConfig = false;
		}
	}

	async function revertToGlobal() {
		if (!agentId || !configServer) return;
		overrideEnabled = false;
		configOverrideRaw = '';
		await saveConfigOverride();
	}

	/**
	 * A plugin only displays an Auth/Config button if the plugin explicitly
	 * requests configuration in its manifest, has global configuration configured,
	 * or has an active agent override.
	 */
	function hasAuthRequired(srv: McpServer): boolean {
		// 1. If this agent already has an explicit override set for this plugin, show it
		if (agentConfigs[srv.id]) return true;

		// 2. If the plugin has global auth / headers / env configured
		if (srv.headers_or_env && srv.headers_or_env.trim().length > 0) return true;

		// 3. If the plugin's manifest explicitly asks for auth or configuration
		if (srv.manifest_json) {
			try {
				const m = JSON.parse(srv.manifest_json);
				if (Array.isArray(m.auth?.fields) && m.auth.fields.length > 0) return true;
				if (m.auth && typeof m.auth === 'object' && Object.keys(m.auth).length > 0) return true;
				if (m.config_schema && typeof m.config_schema === 'object' && Object.keys(m.config_schema).length > 0) return true;
				if (m.config && typeof m.config === 'object' && Object.keys(m.config).length > 0) return true;
				if (Array.isArray(m.required_env) && m.required_env.length > 0) return true;
				if (m.requires_auth === true || m.auth_required === true) return true;
			} catch {}
		}

		return false;
	}

	$effect(() => {
		if (agentId && selectedIds.length > 0) {
			loadToolsAndResources();
		}
	});

	async function loadToolsAndResources() {
		if (!agentId) return;
		isLoadingTools = true;
		try {
			const res = await api.agents.getTools(agentId);
			agentTools = res.tools || [];
			agentResources = res.resources || [];
			subscriptions = res.subscriptions || [];
		} catch (e) {
			console.error('Failed to load agent MCP tools:', e);
		} finally {
			isLoadingTools = false;
		}
	}

	function toggleServer(id: string) {
		if (selectedIds.includes(id)) {
			selectedIds = selectedIds.filter((i) => i !== id);
			if (expandedServerId === id) expandedServerId = null;
		} else {
			selectedIds = [...selectedIds, id];
			expandedServerId = id;
		}
		onchange?.(selectedIds);
		if (agentId) {
			setTimeout(loadToolsAndResources, 650);
		}
	}

	async function toggleTool(tool: AgentToolItem) {
		if (!agentId) return;
		const nextState = !tool.is_enabled;
		tool.is_enabled = nextState;
		try {
			await api.agents.setToolPermission(agentId, tool.mcp_id, tool.name, nextState);
		} catch (e) {
			tool.is_enabled = !nextState; // Revert on failure
			alert('Failed to update tool permission: ' + e);
		}
	}

	async function toggleSubscription(uri: string) {
		if (!agentId) return;
		let updated = [...subscriptions];
		if (updated.includes(uri)) {
			updated = updated.filter((u) => u !== uri);
		} else {
			updated.push(uri);
		}
		subscriptions = updated;

		try {
			await api.agents.setSubscriptions(agentId, updated);
		} catch (e) {
			subscriptions = subscriptions.includes(uri)
				? subscriptions.filter((u) => u !== uri)
				: [...subscriptions, uri];
			alert('Failed to update subscriptions: ' + e);
		}
	}

	async function toggleGlobalPluginActive(srv: McpServer, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		const nextState = srv.is_active === false ? true : false;
		srv.is_active = nextState;
		try {
			await api.mcp.update(srv.id, { is_active: nextState });
			if (agentId) {
				setTimeout(loadToolsAndResources, 350);
			}
		} catch (e) {
			srv.is_active = !nextState;
			alert('Failed to update plugin active status: ' + e);
		}
	}
</script>

<div class="space-y-3">
	{#if allServers.length === 0}
		<div class="rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 p-4 text-center text-xs text-zinc-400 dark:text-zinc-500">
			No MCP servers configured yet. Go to <a href="/mcp" class="text-zinc-900 dark:text-white underline font-medium">MCP Settings</a> to add one.
		</div>
	{:else}
		<div class="space-y-2">
			{#each allServers as srv}
				{@const isSelected = selectedIds.includes(srv.id)}
				{@const isExpanded = expandedServerId === srv.id}
				{@const serverTools = agentTools.filter((t) => t.mcp_id === srv.id)}
				{@const serverResources = agentResources.filter((r) => r.mcp_id === srv.id)}
<!--
`ui://` resources are MCP-*App views* (renderable interfaces displayed in
the Apps sidebar); they are NOT push context. Subscribing to one would
inject the whole HTML document into the model on every turn, so they are
never offered here. Everything else is a genuine push subscription.
-->
{@const serverPushResources = serverResources.filter((r) => !isUiResource(r))}

				<div class="rounded-2xl border transition-all {srv.is_active === false ? 'opacity-70 bg-zinc-50/50 dark:bg-zinc-900/30' : ''} {isSelected
					? 'border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900/70 shadow-xs'
					: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50/70 dark:hover:bg-zinc-800/40'}">
					<!-- Server Header Pill -->
					<div class="flex items-center justify-between p-3">
						<label class="flex flex-1 cursor-pointer items-center gap-3 truncate">
							<input
								type="checkbox"
								checked={isSelected}
								onchange={() => toggleServer(srv.id)}
								class="h-4 w-4 rounded border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 text-zinc-900 dark:text-white focus:ring-0"
							/>
							<div class="truncate">
								<div class="flex items-center gap-2">
									<span class="font-medium text-xs text-zinc-900 dark:text-zinc-100 truncate">{srv.name}</span>
									<span class="rounded px-1.5 py-0.2 text-[9px] font-mono uppercase {srv.transport === 'binary'
										? 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border border-purple-500/20'
										: srv.transport === 'stdio'
										? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
										: 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400'}">
										{srv.transport}
									</span>
								</div>
								<div class="text-[11px] text-zinc-400 dark:text-zinc-500 font-mono truncate max-w-[280px]" title={srv.url_or_command}>
									{srv.url_or_command}
								</div>
							</div>
						</label>

						<div class="flex items-center gap-2">
							<button
								type="button"
								role="switch"
								aria-checked={srv.is_active !== false}
								onclick={(e) => toggleGlobalPluginActive(srv, e)}
								class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-lg border border-zinc-200 dark:border-zinc-700/80 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors text-[10px] cursor-pointer"
								title={srv.is_active !== false ? 'Plugin is active — click to turn off' : 'Plugin is turned off — click to turn on'}
							>
								<span class="h-1.5 w-1.5 rounded-full {srv.is_active !== false ? 'bg-emerald-500' : 'bg-zinc-400'}"></span>
								<span class="text-zinc-600 dark:text-zinc-400 font-medium">{srv.is_active !== false ? 'Active' : 'Off'}</span>
							</button>

							{#if isSelected}
								{#if agentId && hasAuthRequired(srv)}
									<button
										type="button"
										class="flex items-center gap-1.5 rounded-lg border px-2 py-1 text-[11px] font-medium transition-colors cursor-pointer {agentConfigs[srv.id]
											? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
											: 'border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-700'}"
										onclick={() => openConfigModal(srv)}
										title="Configure credentials and environment overrides for this agent"
									>
										<Key class="h-3 w-3" />
										<span>{agentConfigs[srv.id] ? 'Custom Auth' : 'Global Auth'}</span>
									</button>
								{/if}

								{#if isLoadingTools}
									<div class="flex items-center gap-1.5 px-2.5 py-1 text-[11px] font-medium text-zinc-400">
										<RefreshCw class="h-3 w-3 animate-spin text-zinc-400" />
										<span>Loading...</span>
									</div>
								{:else}
									<button
										type="button"
										class="flex items-center gap-1.5 rounded-lg border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-800 px-2.5 py-1 text-[11px] font-medium {serverTools.length > 0 ? 'text-zinc-600 dark:text-zinc-300' : 'text-amber-600 dark:text-amber-400'} hover:bg-zinc-100 dark:hover:bg-zinc-700 transition-colors"
										onclick={() => (expandedServerId = isExpanded ? null : srv.id)}
									>
										<Sliders class="h-3 w-3" />
										<span>{isExpanded ? 'Hide Tools' : `Tools & Push (${serverTools.length})`}</span>
										{#if isExpanded}
											<ChevronUp class="h-3 w-3" />
										{:else}
											<ChevronDown class="h-3 w-3" />
										{/if}
									</button>
								{/if}
							{/if}
						</div>
					</div>

					<!-- Granular Tool & Push MCP Subscription Manager -->
					{#if isSelected && isExpanded}
						<div class="border-t border-zinc-100 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/30 p-3.5 space-y-3.5 rounded-b-2xl">
							{#if srv.is_active === false}
								<div class="p-2.5 rounded-xl bg-amber-500/10 border border-amber-500/20 text-amber-700 dark:text-amber-300 text-xs flex items-center justify-between">
									<span>This plugin is currently turned off. Its tools are paused and will not execute during agent turns.</span>
									<button
										type="button"
										class="underline font-semibold ml-2 text-amber-800 dark:text-amber-200 hover:opacity-80 cursor-pointer"
										onclick={(e) => toggleGlobalPluginActive(srv, e)}
									>
										Turn On
									</button>
								</div>
							{/if}
							<!-- 1. Granular Tool Permissions -->
							<div>
								<div class="flex items-center justify-between mb-2">
									<div class="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
										<ShieldCheck class="h-3.5 w-3.5 text-indigo-500" />
										<span>Granular Tool Permissions (Zero-Trust Discovery)</span>
									</div>
									<div class="flex items-center gap-2">
										<button
											type="button"
											onclick={loadToolsAndResources}
											class="flex items-center gap-1 text-[10px] text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 transition-colors"
											title="Refresh discovered tools from MCP server"
										>
											<RefreshCw class="h-2.5 w-2.5" />
											<span>Refresh</span>
										</button>
										<span class="text-[10px] text-zinc-400">
											Disabled tools are completely hidden from agent
										</span>
									</div>
								</div>

								{#if serverTools.length === 0}
									<div class="rounded-xl border border-dashed border-amber-500/30 bg-amber-500/5 p-3 text-[11px] text-amber-600 dark:text-amber-400 space-y-1">
										<p class="font-medium">No tools discovered for this server.</p>
										<p class="text-[10px] text-zinc-500 dark:text-zinc-400">
											Ensure the MCP server is actively running at <span class="font-mono">{srv.url_or_command}</span> and authentication headers (e.g. <span class="font-mono">Authorization: Bearer &lt;key&gt;</span>) are configured in <a href="/mcp" class="underline font-medium text-zinc-700 dark:text-zinc-200">MCP Settings</a>.
										</p>
									</div>
								{:else}
									<div class="grid grid-cols-1 gap-2">
										{#each serverTools as tool}
											<div class="flex items-center justify-between rounded-xl border border-zinc-200/80 dark:border-zinc-800/80 bg-white dark:bg-zinc-900/60 p-2.5 shadow-2xs">
												<div class="space-y-0.5 max-w-[70%]">
													<div class="flex items-center gap-2">
														<span class="text-xs font-mono font-medium text-zinc-900 dark:text-zinc-100">{tool.name}</span>
														{#if tool.unique_name}
															<span class="rounded px-1.5 py-0.2 text-[9px] font-mono text-zinc-500 bg-zinc-100 dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700" title="Callable unique name">
																{tool.unique_name}
															</span>
														{/if}
														{#if tool.is_enabled}
															<span class="rounded px-1.5 py-0.2 text-[9px] font-mono bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
																Discoverable
															</span>
														{:else}
															<span class="inline-flex items-center gap-1 rounded px-1.5 py-0.2 text-[9px] font-mono bg-zinc-200 dark:bg-zinc-800 text-zinc-500">
																<Lock class="h-2.5 w-2.5" /> Hidden
															</span>
														{/if}
													</div>
													{#if tool.description}
														<p class="text-[11px] text-zinc-500 dark:text-zinc-400 truncate" title={tool.description}>
															{tool.description}
														</p>
													{/if}
												</div>

												<!-- Enable/Disable Switch -->
												<button
													type="button"
													aria-label="Toggle tool discovery permission"
													class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {tool.is_enabled
														? 'bg-emerald-500'
														: 'bg-zinc-300 dark:bg-zinc-700'}"
													onclick={() => toggleTool(tool)}
												>
													<span
														class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {tool.is_enabled
															? 'translate-x-4'
															: 'translate-x-0'}"
													></span>
												</button>
											</div>
										{/each}
									</div>
								{/if}
							</div>

							<!-- 2. Push MCP Subscriptions (model context only) -->
							{#if serverPushResources.length > 0}
								<div class="pt-2 border-t border-zinc-200/60 dark:border-zinc-800/60">
									<div class="flex items-center justify-between mb-2">
										<div class="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-cyan-600 dark:text-cyan-400">
											<RadioTower class="h-3.5 w-3.5 text-cyan-500" />
											<span>Push MCP Subscriptions (Unprompted Model Context)</span>
										</div>
										<span class="text-[10px] text-zinc-400">
											Pushes into model context without calling tools
										</span>
									</div>

									<div class="space-y-2">
										{#each serverPushResources as res}
											{@const isSub = subscriptions.includes(res.uri)}
											<div class="flex items-center justify-between rounded-xl border border-cyan-500/20 bg-cyan-500/5 p-2.5 shadow-2xs">
												<div class="space-y-0.5 max-w-[70%]">
													<div class="flex items-center gap-2">
														<span class="text-xs font-semibold text-zinc-900 dark:text-zinc-100">{res.name}</span>
														<span class="rounded px-1.5 py-0.2 text-[9px] font-mono {isSub ? 'bg-cyan-500/20 text-cyan-700 dark:text-cyan-300 font-bold' : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-500'}">
															{isSub ? 'Subscribed' : 'Not Subscribed'}
														</span>
													</div>
													<div class="text-[10px] font-mono text-zinc-500 truncate" title={res.uri}>
														{res.uri}
													</div>
													{#if res.description}
														<p class="text-[11px] text-zinc-500 dark:text-zinc-400 truncate" title={res.description}>
															{res.description}
														</p>
													{/if}
												</div>

												<!-- Subscribe Toggle Switch -->
												<button
													type="button"
													aria-label="Toggle MCP resource subscription"
													class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {isSub
														? 'bg-cyan-500'
														: 'bg-zinc-300 dark:bg-zinc-700'}"
													onclick={() => toggleSubscription(res.uri)}
												>
													<span
														class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {isSub
															? 'translate-x-4'
															: 'translate-x-0'}"
													></span>
												</button>
											</div>
										{/each}
									</div>
								</div>
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	{#if configModalOpen && configServer}
		<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4 animate-in fade-in duration-150">
			<div class="w-full max-w-lg rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 shadow-2xl overflow-hidden flex flex-col max-h-[90vh]">
				<!-- Modal Header -->
				<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 p-4">
					<div class="flex items-center gap-2.5">
						<div class="p-2 rounded-xl bg-indigo-500/10 text-indigo-500">
							<Key class="h-4 w-4" />
						</div>
						<div>
							<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100 flex items-center gap-2">
								<span>{configServer.name}</span>
								<span class="rounded px-1.5 py-0.2 text-[9px] font-mono uppercase bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
									{configServer.transport}
								</span>
							</h3>
							<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
								Per-agent authentication & environment overrides
							</p>
						</div>
					</div>
					<button
						type="button"
						onclick={closeConfigModal}
						class="p-1 rounded-lg text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
					>
						<X class="h-4 w-4" />
					</button>
				</div>

				<!-- Modal Body -->
				<div class="p-4 space-y-4 overflow-y-auto flex-1 text-xs">
					<!-- Mode Switcher -->
					<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-50/50 dark:bg-zinc-950/40 space-y-2">
						<div class="flex items-center justify-between">
							<div>
								<div class="font-medium text-zinc-900 dark:text-zinc-100">Agent Configuration Override</div>
								<div class="text-[11px] text-zinc-500 dark:text-zinc-400">
									Override the global settings configured in the Plugins menu
								</div>
							</div>
							<button
								type="button"
								role="switch"
								aria-label="Toggle agent configuration override"
								aria-checked={overrideEnabled}
								class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {overrideEnabled
									? 'bg-emerald-500'
									: 'bg-zinc-300 dark:bg-zinc-700'}"
								onclick={() => (overrideEnabled = !overrideEnabled)}
							>
								<span
									class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {overrideEnabled
										? 'translate-x-4'
										: 'translate-x-0'}"
								></span>
							</button>
						</div>

						<div class="pt-2 border-t border-zinc-200/60 dark:border-zinc-800/60 text-[11px]">
							{#if !overrideEnabled}
								<div class="flex items-center gap-1.5 text-zinc-600 dark:text-zinc-400">
									<Globe class="h-3.5 w-3.5 text-indigo-500" />
									<span>Inheriting default configuration from global <strong>Plugins</strong> menu.</span>
								</div>
							{:else}
								<div class="flex items-center gap-1.5 text-emerald-600 dark:text-emerald-400 font-medium">
									<Bot class="h-3.5 w-3.5 text-emerald-500" />
									<span>Custom agent override active. Values defined here take precedence for this agent.</span>
								</div>
							{/if}
						</div>
					</div>

					<!-- Override Editor -->
					{#if overrideEnabled}
						<div class="space-y-2">
							<label for="agent-mcp-override-input" class="block font-medium text-zinc-700 dark:text-zinc-300">
								{configServer.transport === 'http'
									? 'Custom HTTP Headers / Auth'
									: configServer.transport === 'binary'
									? 'Custom Environment Variables / Args'
									: 'Custom Environment Variables'}
							</label>
							<p class="text-[11px] text-zinc-400 dark:text-zinc-500">
								Accepts JSON object (e.g. <span class="font-mono">{`{"API_TOKEN": "secret"}`}</span>) or KEY=VALUE lines.
							</p>
							<textarea
								id="agent-mcp-override-input"
								bind:value={configOverrideRaw}
								rows="5"
								placeholder={configServer.transport === 'http'
									? 'Authorization: Bearer my-agent-token\nX-Custom: value'
									: 'API_KEY=my_agent_api_key\nPORT=8080'}
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-950 px-3 py-2 font-mono text-xs text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 focus:border-indigo-500 focus:outline-none"
							></textarea>
						</div>
					{/if}

					<!-- Global Default Reference -->
					{#if globalConfigRaw}
						<div class="rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-50/30 dark:bg-zinc-900/30 space-y-1">
							<div class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
								Global Default Reference
							</div>
							<pre class="font-mono text-[10px] text-zinc-500 dark:text-zinc-400 overflow-x-auto whitespace-pre-wrap">{globalConfigRaw}</pre>
						</div>
					{/if}
				</div>

				<!-- Modal Footer -->
				<div class="flex items-center justify-between border-t border-zinc-100 dark:border-zinc-800 p-3 bg-zinc-50/50 dark:bg-zinc-950/20">
					<div>
						{#if overrideEnabled || agentConfigs[configServer.id]}
							<button
								type="button"
								onclick={revertToGlobal}
								disabled={isSavingConfig}
								class="text-xs text-amber-600 dark:text-amber-400 hover:underline cursor-pointer disabled:opacity-50"
							>
								Revert to Global Default
							</button>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						<button
							type="button"
							onclick={closeConfigModal}
							class="px-3 py-1.5 rounded-xl border border-zinc-200 dark:border-zinc-700 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
						>
							Cancel
						</button>
						<button
							type="button"
							onclick={saveConfigOverride}
							disabled={isSavingConfig}
							class="px-3.5 py-1.5 rounded-xl bg-zinc-900 dark:bg-white text-white dark:text-zinc-900 text-xs font-semibold hover:opacity-90 transition-opacity disabled:opacity-50 cursor-pointer"
						>
							{isSavingConfig ? 'Saving...' : 'Save Configuration'}
						</button>
					</div>
				</div>
			</div>
		</div>
	{/if}
</div>
