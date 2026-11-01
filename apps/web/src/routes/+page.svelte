<script lang="ts">
	import { page } from '$app/state';
	import { api, type Agent, type Provider, type McpServer, type AgentDetail, type ExtensionItem, type ExtensionUiHook, type ResponseHook } from '$lib/api';
	import { authState } from '$lib/stores/auth.svelte';
	import { eventStore } from '$lib/stores/events.svelte';
	import TerminalLog from '$lib/components/TerminalLog.svelte';
	import ProviderSelect from '$lib/components/ProviderSelect.svelte';
	import McpList from '$lib/components/McpList.svelte';
	import ExtensionChatPanel from '$lib/components/ExtensionChatPanel.svelte';
	import {
		HelpCircle,
		CheckCircle2,
		Play,
		Send,
		AlertCircle,
		Cpu,
		Radio,
		Layers,
		BrainCircuit,
		Eye,
		History,
		Terminal,
		Trash2,
		Sparkles,
		Bot,
		Plus,
		CornerDownLeft,
		MessageSquare,
		FileCode
	} from 'lucide-svelte';

	let agentId = $derived(page.url.searchParams.get('agent_id'));

	let agent = $state<Agent | null>(null);
	let selectedProviderIds = $state<string[]>([]);
	let selectedMcpIds = $state<string[]>([]);
	let allProviders = $state<Provider[]>([]);
	let allMcpServers = $state<McpServer[]>([]);

	let agentMemory = $state<{ content: string; current_hash: string; character_count: number } | null>(null);
	let showMemoryPreview = $state(false);

	let bottomTab = $state<'terminal' | 'history'>('terminal');
	let runningHistory = $state<any[]>([]);

	let maxRunningHistoryChars = $state<number>(300000);
	let responseHooks = $state<ResponseHook[]>([]);

	let currentHistoryChars = $derived(
		runningHistory.reduce((acc, item) => {
			const cLen = (item.content || '').length;
			const tcLen = (item.tool_calls_json || '').length;
			return acc + cLen + tcLen;
		}, 0)
	);

	let isSaving = $state(false);
	let saveIndicator = $state(false);
	let saveTimeout: number | null = null;
	let debounceTimer: number | null = null;

	let userInput = $state('');
	let isRunning = $state(false);

	// Zero state quick creation
	let newAgentName = $state('');
	let isCreating = $state(false);

	// Navigation & Extensions
	let activePanel = $state<'config' | string>('config');
	let activeAnchor = $state<string>('agent-settings-section');
	let extensions = $state<ExtensionItem[]>([]);
	let selectedExtension = $state<ExtensionItem | null>(null);
	let selectedHook = $state<ExtensionUiHook | null>(null);

	let sidebarExtensions = $derived(
		extensions
			.filter((e) => e.is_active && e.manifest?.ui_hooks?.some((h) => h.target_id === 'agent-sidebar-nav'))
			.map((e) => ({
				...e,
				hooks: (e.manifest?.ui_hooks || []).filter((h) => h.target_id === 'agent-sidebar-nav')
			}))
	);

	async function loadExtensions() {
		try {
			extensions = await api.extensions.list();
		} catch (e) {
			console.error('Failed to load extensions:', e);
		}
	}

	function selectAnchor(anchorId: string) {
		activePanel = 'config';
		activeAnchor = anchorId;
		setTimeout(() => {
			const el = document.getElementById(anchorId);
			if (el) {
				el.scrollIntoView({ behavior: 'smooth', block: 'start' });
			}
		}, 20);
	}

	function selectExtensionHook(ext: ExtensionItem, hook: ExtensionUiHook) {
		selectedExtension = ext;
		selectedHook = hook;
		activePanel = `ext:${ext.id}:${hook.panel_id || 'default'}`;
	}

	// Load data when agentId changes
	$effect(() => {
		if (agentId) {
			loadAgentData(agentId);
			eventStore.setAgent(agentId);
		} else {
			agent = null;
			eventStore.setAgent(null);
		}
	});

	async function loadAgentData(id: string) {
		try {
			const [detail, provs, mcps, mem, hist] = await Promise.all([
				api.agents.get(id),
				api.providers.list(),
				api.mcp.list(),
				api.agents.getMemory(id).catch(() => null),
				api.agents.getHistory(id).catch(() => [])
			]);
			agent = detail.agent;
			selectedProviderIds = detail.model_ids || detail.provider_ids || [];
			selectedMcpIds = detail.mcp_ids;
			allProviders = provs;
			allMcpServers = mcps;
			agentMemory = mem;
			runningHistory = hist;
			maxRunningHistoryChars = agent.max_running_history_chars || 300000;
			if (agent.response_hooks_json) {
				try {
					const parsed = JSON.parse(agent.response_hooks_json);
					if (Array.isArray(parsed)) {
						responseHooks = parsed.map((item, idx) => {
							if (typeof item === 'string') {
								return { id: `hook_${idx}`, message: item, source: 'user', enabled: true };
							}
							return {
								id: item.id || `hook_${idx}`,
								message: item.message || '',
								source: item.source || 'user',
								enabled: item.enabled ?? true
							};
						});
					} else {
						responseHooks = [];
					}
				} catch {
					responseHooks = [];
				}
			} else {
				responseHooks = [];
			}
			loadExtensions();
		} catch (e) {
			console.error('Failed to load agent detail:', e);
		}
	}

	async function reloadHistory() {
		if (!agent) return;
		try {
			runningHistory = await api.agents.getHistory(agent.id);
			agentMemory = await api.agents.getMemory(agent.id).catch(() => null);
		} catch (e) {
			console.error('Failed to reload history:', e);
		}
	}

	async function clearHistory() {
		if (!agent) return;
		if (!confirm('Clear all conversation history for this agent?')) return;
		try {
			await api.agents.clearHistory(agent.id);
			runningHistory = [];
		} catch (e) {
			alert('Failed to clear history: ' + e);
		}
	}

	function triggerAutoSave() {
		if (!agent) return;
		if (debounceTimer) clearTimeout(debounceTimer);

		isSaving = true;
		debounceTimer = window.setTimeout(async () => {
			if (!agent) return;
			try {
				const updated = await api.agents.patch(agent.id, {
					name: agent.name,
					strategy: agent.strategy,
					system_prompt: agent.system_prompt,
					is_active: agent.is_active,
					model_ids: selectedProviderIds,
					provider_ids: selectedProviderIds,
					mcp_ids: selectedMcpIds,
					allow_output_text: agent.allow_output_text,
					output_text_error_message: agent.output_text_error_message,
					max_running_history_chars: Number(maxRunningHistoryChars) || 300000,
					response_hooks: responseHooks
				});
				agent = updated;
				showSavedBadge();
			} catch (e) {
				console.error('Auto-save error:', e);
			} finally {
				isSaving = false;
			}
		}, 500);
	}

	function addResponseHook() {
		responseHooks = [
			...responseHooks,
			{
				id: 'hook_' + Math.random().toString(36).substring(2, 9),
				message: '',
				source: 'user',
				enabled: true
			}
		];
		triggerAutoSave();
	}

	function removeResponseHook(id: string) {
		responseHooks = responseHooks.filter((h) => h.id !== id);
		triggerAutoSave();
	}

	function toggleResponseHook(id: string) {
		const hook = responseHooks.find((h) => h.id === id);
		if (hook) {
			hook.enabled = !hook.enabled;
			triggerAutoSave();
		}
	}

	function showSavedBadge() {
		saveIndicator = true;
		if (saveTimeout) clearTimeout(saveTimeout);
		saveTimeout = window.setTimeout(() => {
			saveIndicator = false;
		}, 1800);
	}

	async function runTurn() {
		if (!agent || !userInput.trim() || isRunning) return;
		const input = userInput.trim();
		userInput = '';
		isRunning = true;
		try {
			await api.agents.run(agent.id, input);
			setTimeout(reloadHistory, 1500);
		} catch (e) {
			alert('Failed to execute agent turn: ' + e);
		} finally {
			isRunning = false;
		}
	}

	async function createAgentFromZero() {
		if (!newAgentName.trim() || isCreating) return;
		isCreating = true;
		try {
			const created = await api.agents.create({
				name: newAgentName.trim(),
				strategy: 'fallback',
				system_prompt: 'You are {{name}}, an autonomous AI assistant.\nAvailable tools: {{tools-preset}}'
			});
			window.location.href = `/?agent_id=${created.id}`;
		} catch (e) {
			alert('Failed to create agent: ' + e);
		} finally {
			isCreating = false;
		}
	}
</script>

{#if !agentId || !agent}
	<!-- 🎨 CLEAN MINIMALIST ZERO STATE (Grok feel) -->
	<div id="agent-zero-state" class="flex h-full flex-col items-center justify-center px-6 py-12 max-w-xl mx-auto w-full text-center">
		<!-- Minimalist Brand Header -->
		<div class="space-y-2 mb-8">
			<h1 class="text-3xl font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">
				NeuxDOT
			</h1>
			<p class="text-sm text-zinc-500 dark:text-zinc-400 max-w-md">
				Autonomous, cache-invariant, MCP-native agent runtime.
			</p>
		</div>

		<!-- Clean Quick Creation Pill -->
		<div class="w-full max-w-sm space-y-3">
			<div class="flex items-center gap-2 rounded-full border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900/80 p-1.5 pl-4 shadow-sm focus-within:ring-2 focus-within:ring-zinc-400 dark:focus-within:ring-zinc-600 transition-all">
				<input
					type="text"
					bind:value={newAgentName}
					placeholder="Name a new agent..."
					class="flex-1 bg-transparent text-xs text-zinc-900 dark:text-white placeholder-zinc-400 focus:outline-none"
					onkeydown={(e) => e.key === 'Enter' && createAgentFromZero()}
				/>
				<button
					type="button"
					class="flex h-8 px-4 items-center justify-center rounded-full bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 text-white dark:text-zinc-950 text-xs font-medium shadow-xs disabled:opacity-40 transition-all active:scale-95"
					onclick={createAgentFromZero}
					disabled={!newAgentName.trim() || isCreating}
				>
					{isCreating ? 'Creating...' : 'Create'}
				</button>
			</div>

			<p class="text-[11px] text-zinc-400 dark:text-zinc-500">
				Select an agent from the left sidebar or type a name to begin.
			</p>
		</div>
	</div>
{:else}
	<!-- 🖥️ ACTIVE AGENT VIEW -->
	<div id="agent-workbench" class="flex h-full flex-col overflow-hidden">
		<!-- Header -->
		<header id="agent-header" class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5">
			<div class="flex items-center gap-3">
				<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">{agent.name}</h1>
				<button
					type="button"
					onclick={() => {
						if (agent) {
							agent.is_active = !agent.is_active;
							triggerAutoSave();
						}
					}}
					class="cursor-pointer rounded-full px-2.5 py-0.5 text-[10px] font-mono transition-all hover:scale-105 active:scale-95 {agent.is_active ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20 hover:bg-emerald-500/20' : 'bg-zinc-200/60 dark:bg-zinc-800 text-zinc-500 hover:bg-zinc-200'}"
					title="Click to toggle active state"
				>
					{agent.is_active ? 'Active' : 'Inactive'}
				</button>
			</div>

			<!-- Auto-save notification badge -->
			<div class="flex items-center gap-2">
				{#if isSaving}
					<span class="text-xs text-zinc-400 animate-pulse">Saving...</span>
				{:else if saveIndicator}
					<span class="inline-flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400 transition-opacity duration-300">
						<CheckCircle2 class="h-3.5 w-3.5" /> Saved
					</span>
				{/if}
			</div>
		</header>

		<!-- Body with Inner Sidebar and Main Panel -->
		<div class="flex flex-1 overflow-hidden">
			<!-- 🧭 Inner Sidebar Navigation -->
			<aside id="agent-sidebar-nav" class="w-56 shrink-0 border-r border-zinc-200/80 dark:border-zinc-800/80 bg-zinc-50/50 dark:bg-zinc-950/40 p-3 flex flex-col justify-between overflow-y-auto">
				<div class="space-y-4">
					<!-- Standard Anchors / Views -->
					<div>
						<div class="px-2 mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
							Configuration
						</div>
						<nav class="space-y-0.5">
							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-settings-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-settings-section')}
							>
								<Layers class="h-3.5 w-3.5 shrink-0" />
								<span>Settings</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-providers-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-providers-section')}
							>
								<Cpu class="h-3.5 w-3.5 shrink-0" />
								<span>Models & Routing</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-prompt-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-prompt-section')}
							>
								<Sparkles class="h-3.5 w-3.5 shrink-0" />
								<span>System Prompt</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-memory-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-memory-section')}
							>
								<BrainCircuit class="h-3.5 w-3.5 shrink-0" />
								<span>Persistent Memory</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-permissions-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-permissions-section')}
							>
								<Radio class="h-3.5 w-3.5 shrink-0" />
								<span>Flow & Output</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-mcp-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-mcp-section')}
							>
								<Play class="h-3.5 w-3.5 shrink-0" />
								<span>Connected MCP</span>
							</button>

							<button
								type="button"
								class="w-full flex items-center justify-between px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-hooks-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-hooks-section')}
							>
								<div class="flex items-center gap-2.5">
									<Layers class="h-3.5 w-3.5 shrink-0" />
									<span>Hooks</span>
								</div>
								{#if responseHooks.length > 0}
									<span class="text-[10px] font-mono px-1.5 py-0.2 rounded-full {activePanel === 'config' && activeAnchor === 'agent-hooks-section' ? 'bg-white/20 text-white' : 'bg-zinc-200 dark:bg-zinc-700 text-zinc-700 dark:text-zinc-300'}">
										{responseHooks.length}
									</span>
								{/if}
							</button>

							<button
								type="button"
								class="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === 'config' && activeAnchor === 'agent-terminal-section' ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900 shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
								onclick={() => selectAnchor('agent-terminal-section')}
							>
								<Terminal class="h-3.5 w-3.5 shrink-0" />
								<span>Terminal & History</span>
							</button>
						</nav>
					</div>

					<!-- Extensions Hooked to agent-sidebar-nav -->
					<div>
						<div class="flex items-center justify-between px-2 mb-1.5">
							<span class="text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
								Extensions
							</span>
							<a
								href="/settings#extensions"
								class="text-[10px] text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 transition-colors"
								title="Install or manage WASM extensions"
							>
								Manage
							</a>
						</div>

						{#if sidebarExtensions.length === 0}
							<div class="px-2 py-2 text-[11px] text-zinc-400 dark:text-zinc-600 italic">
								No sidebar extensions installed
							</div>
						{:else}
							<nav class="space-y-0.5">
								{#each sidebarExtensions as ext}
									{#each ext.hooks as hook}
										{@const panelKey = `ext:${ext.id}:${hook.panel_id || 'default'}`}
										<button
											type="button"
											class="w-full flex items-center justify-between px-2.5 py-1.5 rounded-xl text-xs font-medium transition-all {activePanel === panelKey ? 'bg-indigo-600 text-white shadow-xs' : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'}"
											onclick={() => selectExtensionHook(ext, hook)}
										>
											<div class="flex items-center gap-2 truncate">
												<MessageSquare class="h-3.5 w-3.5 shrink-0" />
												<span class="truncate">{hook.title || ext.name}</span>
											</div>
											<span class="text-[9px] px-1 py-0.2 rounded font-mono uppercase {activePanel === panelKey ? 'bg-white/20 text-white' : 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400'}">
												WASM
											</span>
										</button>
									{/each}
								{/each}
							</nav>
						{/if}
					</div>
				</div>

				<!-- Footer in Sidebar -->
				<div class="pt-3 border-t border-zinc-200/60 dark:border-zinc-800/60">
					<div class="flex items-center justify-between text-[10px] text-zinc-400 px-1">
						<span>Agent ID</span>
						<span class="font-mono text-zinc-500">{agent.id.substring(0, 8)}...</span>
					</div>
				</div>
			</aside>

			<!-- Main Content View -->
			<main class="flex-1 flex flex-col h-full overflow-hidden">
				{#if activePanel === 'config'}
					<!-- Main Scrollable Configuration Area -->
					<div id="agent-config-scroll" class="flex-1 overflow-y-auto px-6 py-6 space-y-5">
			<!-- Settings Form -->
			<section id="agent-settings-section" class="grid grid-cols-1 md:grid-cols-3 gap-4">
				<!-- Name -->
				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
					<label for="agent-name" class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500 mb-1.5">
						Agent Name
					</label>
					<input
						id="agent-name"
						type="text"
						bind:value={agent.name}
						oninput={triggerAutoSave}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-colors"
					/>
				</div>

				<!-- Strategy -->
				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
					<label for="routing-strategy" class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500 mb-1.5">
						Provider Routing Strategy
					</label>
					<select
						id="routing-strategy"
						bind:value={agent.strategy}
						onchange={triggerAutoSave}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-colors"
					>
						<option value="fallback">Fallback (try next on 429/5xx)</option>
						<option value="round-robin">Round-Robin</option>
						<option value="random">Random</option>
					</select>
				</div>

				<!-- Max Running History Characters -->
				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
					<div class="flex items-center justify-between mb-1.5">
						<label for="max-history-chars" class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
							Max History Chars
						</label>
						<span class="text-[10px] font-mono text-zinc-400" title="Running conversation character usage">
							{currentHistoryChars.toLocaleString()} / {Number(maxRunningHistoryChars || 300000).toLocaleString()}
						</span>
					</div>
					<input
						id="max-history-chars"
						type="number"
						min="1000"
						max="10000000"
						step="10000"
						bind:value={maxRunningHistoryChars}
						oninput={triggerAutoSave}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-colors"
					/>
				</div>
			</section>

			<!-- Providers Multi-Select -->
			<section id="agent-providers-section" class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
				<div class="flex items-center justify-between mb-2">
					<span class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
						Connected Models & Providers
					</span>
					<a href="/providers" class="text-xs text-zinc-500 hover:text-zinc-900 dark:hover:text-white underline transition-colors">Manage Providers</a>
				</div>
				<ProviderSelect
					{allProviders}
					bind:selectedIds={selectedProviderIds}
					onchange={() => triggerAutoSave()}
				/>
			</section>

			<!-- System Prompt -->
			<section id="agent-prompt-section" class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
				<div class="flex items-center justify-between mb-2">
					<div class="flex items-center gap-1.5">
						<label for="system-prompt" class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
							System Prompt
						</label>
						<a
							href="/docs/prompt-variables"
							class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white transition-colors"
							title="View prompt variable documentation"
						>
							<HelpCircle class="h-3.5 w-3.5" />
						</a>
					</div>
					<span class="text-[11px] font-mono text-zinc-400">Variables: {'{{agent.name}}'}, {'{{agent.memory}}'}, {'{{tools.preset}}'}</span>
				</div>
				<textarea
					id="system-prompt"
					rows="4"
					bind:value={agent.system_prompt}
					oninput={triggerAutoSave}
					placeholder="You are {'{{agent.name}}'}, an autonomous AI assistant..."
					class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 p-3 text-xs font-mono text-zinc-900 dark:text-zinc-200 placeholder-zinc-400 focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-colors"
				></textarea>
			</section>

			<!-- 🧠 Persistent Memory Status & Meta-Tool Inspection -->
			<section id="agent-memory-section" class="rounded-2xl border border-pink-500/20 bg-pink-500/5 p-4 space-y-2">
				<div class="flex items-center justify-between">
					<div class="flex items-center gap-2">
						<BrainCircuit class="h-4 w-4 text-pink-500 dark:text-pink-400" />
						<span class="text-xs font-semibold uppercase tracking-wider text-pink-700 dark:text-pink-300">
							Persistent Memory
						</span>
						<span class="text-[10px] font-mono px-2 py-0.5 rounded-full bg-pink-500/10 text-pink-700 dark:text-pink-300 border border-pink-500/20">
							{agentMemory?.character_count || 0} / 5,000 chars
						</span>
					</div>

					<div class="flex items-center gap-2">
						{#if agentMemory?.current_hash}
							<span class="text-[10px] font-mono text-zinc-500 dark:text-zinc-400" title={agentMemory.current_hash}>
								Hash: <code class="text-pink-600 dark:text-pink-400">{agentMemory.current_hash.substring(0, 10)}...</code>
							</span>
						{/if}
						<button
							type="button"
							class="flex items-center gap-1 rounded-lg bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 px-2.5 py-1 text-[11px] font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-700 transition-colors shadow-2xs"
							onclick={() => (showMemoryPreview = !showMemoryPreview)}
						>
							<Eye class="h-3 w-3" />
							<span>{showMemoryPreview ? 'Hide Memory' : 'View Memory'}</span>
						</button>
					</div>
				</div>

				<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
					Managed via <code>setMemory</code>, <code>editMemory</code>, <code>readMemory</code>, and <code>compact()</code> meta-tools. Guarded by hash verification and 5k max character limit.
				</p>

				{#if showMemoryPreview}
					<div class="mt-2 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-950 p-3 font-mono text-xs text-zinc-800 dark:text-zinc-300 whitespace-pre-wrap max-h-40 overflow-y-auto">
						{#if agentMemory?.content}
							{agentMemory.content}
						{:else}
							<span class="italic text-zinc-400">Memory is currently empty.</span>
						{/if}
					</div>
				{/if}
			</section>

			<!-- 🛑 Flow Control & Output Text Permissions -->
			<section id="agent-permissions-section" class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 space-y-3 shadow-xs">
				<div class="flex items-center justify-between">
					<div>
						<div class="flex items-center gap-2">
							<span class="text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
								Allow Output Text
							</span>
							<span class="text-[10px] font-mono px-2 py-0.5 rounded-full {agent.allow_output_text ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20' : 'bg-rose-500/10 text-rose-600 dark:text-rose-400 border border-rose-500/20'}">
								{agent.allow_output_text ? 'Allowed' : 'Disallowed (Tool/Sleep Only)'}
							</span>
						</div>
						<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-0.5">
							When disabled, plain text turn outputs are blocked. The agent is forced to use external MCP tools, update memory, or call <code>sleep(duration)</code>.
						</p>
					</div>

					<!-- Apple-style Toggle -->
					<button
						type="button"
						aria-label="Toggle output text permission"
						class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {agent.allow_output_text
							? 'bg-emerald-500'
							: 'bg-zinc-300 dark:bg-zinc-700'}"
						onclick={() => {
							if (agent) {
								agent.allow_output_text = !agent.allow_output_text;
								triggerAutoSave();
							}
						}}
					>
						<span
							class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {agent.allow_output_text
								? 'translate-x-4'
								: 'translate-x-0'}"
						></span>
					</button>
				</div>

				{#if !agent.allow_output_text}
					<div>
						<label for="disallowed-text-msg" class="block text-[11px] font-medium text-zinc-500 dark:text-zinc-400 mb-1">
							Disallowed Output Error Prompt (sent to LLM on plain text)
						</label>
						<input
							id="disallowed-text-msg"
							type="text"
							bind:value={agent.output_text_error_message}
							oninput={triggerAutoSave}
							placeholder="Output Text is not allowed, consider use correct tool or sleep."
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-colors"
						/>
					</div>
				{/if}
			</section>

			<!-- MCP Checklist -->
			<section id="agent-mcp-section" class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
				<div class="flex items-center justify-between mb-2">
					<span class="block text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
						Connected MCP Servers & Tools
					</span>
					<a href="/mcp" class="text-xs text-zinc-500 hover:text-zinc-900 dark:hover:text-white underline transition-colors">Manage MCP Servers</a>
				</div>
				<McpList
					agentId={agent.id}
					allServers={allMcpServers}
					bind:selectedIds={selectedMcpIds}
					onchange={() => triggerAutoSave()}
				/>
			</section>

			<!-- 🪝 Response Hooks Section -->
			<section id="agent-hooks-section" class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 space-y-3 shadow-xs">
				<div class="flex items-center justify-between">
					<div class="flex items-center gap-2">
						<Layers class="h-4 w-4 text-indigo-500 dark:text-indigo-400" />
						<span class="text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
							Response Hooks
						</span>
						<span class="text-[10px] font-mono px-2 py-0.5 rounded-full bg-indigo-500/10 text-indigo-700 dark:text-indigo-300 border border-indigo-500/20">
							{responseHooks.length} active
						</span>
					</div>

					<button
						type="button"
						onclick={addResponseHook}
						class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-zinc-100 dark:hover:bg-zinc-200 text-white dark:text-zinc-900 px-3 py-1.5 text-xs font-medium shadow-xs transition-colors cursor-pointer"
					>
						<Plus class="h-3.5 w-3.5" />
						<span>Add Hook</span>
					</button>
				</div>

				<p class="text-[11px] text-zinc-500 dark:text-zinc-400 leading-relaxed">
					Messages configured here are transparently evaluated and injected into every tool execution result and event returning to the model. Extensions and users can register hooks here.
				</p>

				<!-- Variable Badges -->
				<div class="flex flex-wrap items-center gap-1.5 pt-1">
					<span class="text-[10px] font-medium text-zinc-400 uppercase tracking-wider">Available Variables:</span>
					<button
						type="button"
						class="text-[10px] font-mono px-2 py-0.5 rounded-md bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors border border-zinc-200 dark:border-zinc-700 cursor-pointer"
						title="Click to copy variable"
						onclick={() => navigator.clipboard.writeText('{{time}}')}
					>
						{'{{time}}'}
					</button>
					<button
						type="button"
						class="text-[10px] font-mono px-2 py-0.5 rounded-md bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors border border-zinc-200 dark:border-zinc-700 cursor-pointer"
						title="Click to copy variable"
						onclick={() => navigator.clipboard.writeText('{{context_remaining}}')}
					>
						{'{{context_remaining}}'}
					</button>
				</div>

				<!-- Hooks List -->
				{#if responseHooks.length === 0}
					<div class="rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 p-6 text-center text-xs text-zinc-400 italic">
						No response hooks configured. Click "+ Add Hook" above to inject custom messages or status into tool results.
					</div>
				{:else}
					<div class="space-y-2 pt-1">
						{#each responseHooks as hook (hook.id)}
							<div class="flex items-center gap-2.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50/70 dark:bg-zinc-950/70 p-2.5">
								<!-- Enable Toggle -->
								<input
									type="checkbox"
									checked={hook.enabled}
									onchange={() => toggleResponseHook(hook.id)}
									class="rounded border-zinc-300 text-indigo-600 focus:ring-indigo-500 cursor-pointer"
									title="Enable/disable hook"
								/>

								<!-- Transparent Source Badge -->
								<span class="text-[10px] font-mono px-2 py-0.5 rounded uppercase font-semibold {hook.source.startsWith('extension') ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border border-purple-500/20' : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 border border-zinc-300/50 dark:border-zinc-700'}">
									{hook.source}
								</span>

								<!-- Message Input -->
								<input
									type="text"
									bind:value={hook.message}
									oninput={triggerAutoSave}
									placeholder="e.g. Current time is {'{{time}}'}. Remaining context: {'{{context_remaining}}'}"
									class="flex-1 bg-transparent px-2 py-1 text-xs font-mono text-zinc-900 dark:text-white placeholder-zinc-400 focus:outline-none"
								/>

								<!-- Delete Button -->
								<button
									type="button"
									onclick={() => removeResponseHook(hook.id)}
									class="p-1 rounded-lg text-zinc-400 hover:text-rose-500 hover:bg-rose-500/10 transition-colors cursor-pointer"
									title="Remove hook"
								>
									<Trash2 class="h-3.5 w-3.5" />
								</button>
							</div>
						{/each}
					</div>
				{/if}
			</section>

			<!-- 💬 FLOATING PROMPT PILL BAR (Reference Image 2 / Grok Feel) -->
			<div id="agent-prompt-bar" class="sticky bottom-4 z-20">
				<div class="flex items-center gap-2 rounded-full border border-zinc-200 dark:border-zinc-800 bg-white/90 dark:bg-zinc-900/90 backdrop-blur-xl p-2 pl-4 shadow-lg transition-all focus-within:ring-2 focus-within:ring-zinc-400 dark:focus-within:ring-zinc-600">
					<!-- Strategy Pill -->
					<div class="flex items-center gap-1.5 rounded-full bg-zinc-100 dark:bg-zinc-800 px-3 py-1 text-[11px] font-medium text-zinc-600 dark:text-zinc-300">
						<Sparkles class="h-3 w-3 text-indigo-500" />
						<span class="capitalize">{agent.strategy}</span>
					</div>

					<input
						id="turn-prompt"
						type="text"
						bind:value={userInput}
						placeholder="What do you want to instruct {agent.name} to do?"
						class="flex-1 bg-transparent px-2 text-xs text-zinc-900 dark:text-white placeholder-zinc-400 focus:outline-none"
						onkeydown={(e) => e.key === 'Enter' && runTurn()}
					/>

					<button
						type="button"
						class="flex h-8 w-8 items-center justify-center rounded-full bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 text-white dark:text-zinc-950 disabled:opacity-30 shadow-xs transition-all active:scale-95"
						onclick={runTurn}
						disabled={!userInput.trim() || isRunning}
						aria-label="Send prompt"
					>
						{#if isRunning}
							<div class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white dark:border-zinc-900 border-t-transparent"></div>
						{:else}
							<CornerDownLeft class="h-4 w-4" />
						{/if}
					</button>
				</div>
			</div>

			<!-- Bottom Area: Terminal Log & Running History -->
			<section id="agent-terminal-section">
				<div class="flex items-center justify-between mb-2">
					<div class="flex items-center gap-2">
						<button
							type="button"
							class="flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition-colors {bottomTab === 'terminal' ? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-xs' : 'text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200'}"
							onclick={() => (bottomTab = 'terminal')}
						>
							<Terminal class="h-3.5 w-3.5" />
							<span>Live Terminal</span>
						</button>
						<button
							type="button"
							class="flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition-colors {bottomTab === 'history' ? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-xs' : 'text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200'}"
							onclick={() => {
								bottomTab = 'history';
								reloadHistory();
							}}
						>
							<History class="h-3.5 w-3.5" />
							<span>Running History ({runningHistory.length})</span>
						</button>
					</div>

					{#if bottomTab === 'history' && runningHistory.length > 0}
						<button
							type="button"
							class="flex items-center gap-1 text-[11px] text-zinc-400 hover:text-red-500 transition-colors"
							onclick={clearHistory}
						>
							<Trash2 class="h-3 w-3" />
							<span>Clear History</span>
						</button>
					{/if}
				</div>

				{#if bottomTab === 'terminal'}
					<TerminalLog />
				{:else}
					<div class="h-72 overflow-y-auto rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-950 p-4 font-mono text-xs space-y-3 shadow-xs">
						{#if runningHistory.length === 0}
							<div class="flex h-full items-center justify-center text-zinc-400 dark:text-zinc-600 italic">
								No running history recorded yet. Execute an agent turn to start history.
							</div>
						{:else}
							{#each runningHistory as msg (msg.id)}
								<div class="rounded-xl border p-3 {msg.role === 'user' ? 'border-indigo-500/20 bg-indigo-500/5' : msg.role === 'assistant' ? 'border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900/60' : msg.role === 'tool' ? 'border-emerald-500/20 bg-emerald-500/5' : 'border-zinc-200 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-900/30'}">
									<div class="flex items-center justify-between mb-1">
										<span class="rounded px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wider {msg.role === 'user' ? 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400' : msg.role === 'assistant' ? 'bg-zinc-200 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300' : msg.role === 'tool' ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400' : 'bg-zinc-200 text-zinc-600'}">
											{msg.role}
										</span>
										{#if msg.tool_call_id}
											<span class="text-[10px] text-zinc-400 font-mono">id: {msg.tool_call_id}</span>
										{/if}
									</div>
									<div class="text-zinc-800 dark:text-zinc-200 whitespace-pre-wrap select-text leading-relaxed">{msg.content}</div>
									{#if msg.tool_calls_json}
										<pre class="mt-2 text-[10px] text-zinc-600 dark:text-zinc-400 bg-zinc-100 dark:bg-zinc-950 p-2 rounded-lg overflow-x-auto border border-zinc-200/80 dark:border-zinc-800">{msg.tool_calls_json}</pre>
									{/if}
								</div>
							{/each}
						{/if}
					</div>
				{/if}
			</section>
		</div>
		{:else if selectedExtension && selectedHook}
			<ExtensionChatPanel
				{agent}
				extension={selectedExtension}
				hook={selectedHook}
			/>
		{/if}
		</main>
	</div>
</div>
{/if}
