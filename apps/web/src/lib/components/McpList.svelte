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
		RefreshCw
	} from 'lucide-svelte';

	interface Props {
		agentId?: string;
		allServers: McpServer[];
		selectedIds: string[];
		onchange?: (selected: string[]) => void;
	}

	let { agentId = '', allServers = [], selectedIds = $bindable([]), onchange }: Props = $props();

	let agentTools = $state<AgentToolItem[]>([]);
	let agentResources = $state<AgentResourceItem[]>([]);
	let subscriptions = $state<string[]>([]);
	let expandedServerId = $state<string | null>(null);
	let isLoadingTools = $state(false);

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
				{@const isWasm = srv.transport === 'wasm_stdio' || srv.url_or_command.startsWith('wasm://')}

				<div class="rounded-2xl border transition-all {isSelected
					? 'border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900/70 shadow-xs'
					: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50/70 dark:hover:bg-zinc-850/40'}">
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
									<span class="rounded px-1.5 py-0.2 text-[9px] font-mono uppercase {isWasm
										? 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20'
										: srv.transport === 'stdio'
										? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
										: 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400'}">
										{isWasm ? 'WASM stdio' : srv.transport}
									</span>
								</div>
								<div class="text-[11px] text-zinc-400 dark:text-zinc-500 font-mono truncate max-w-[280px]" title={srv.url_or_command}>
									{srv.url_or_command}
								</div>
							</div>
						</label>

						{#if isSelected}
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

					<!-- Granular Tool & Push MCP Subscription Manager -->
					{#if isSelected && isExpanded}
						<div class="border-t border-zinc-100 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/30 p-3.5 space-y-3.5 rounded-b-2xl">
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

							<!-- 2. Push MCP Subscriptions -->
							{#if serverResources.length > 0}
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
										{#each serverResources as res}
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
</div>
