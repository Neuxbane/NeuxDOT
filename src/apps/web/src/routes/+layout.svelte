<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { page } from '$app/state';
	import { api, type Agent, type ServerAccount } from '$lib/api';
	import { authState } from '$lib/stores/auth.svelte';
	import { themeStore } from '$lib/stores/theme.svelte';
	import { navState } from '$lib/stores/nav.svelte';
	import Avatar from '$lib/components/Avatar.svelte';
	import {
		Plus,
		Search,
		Cpu,
		Radio,
		Settings,
		Sun,
		Moon,
		Bot,
		Trash2,
		Menu,
		X,
		ArrowLeft
	} from 'lucide-svelte';

	let { children } = $props();

	let searchQuery = $state('');
	// Mobile: the navigation sidebar is an off-canvas drawer below `lg`.
	// Mobile drill-down, level 1: selecting an agent or nav item hides this and
	// shows the next screen. The back control on that screen returns here.
	const sidebarOpen = $derived(navState.level1);
	let agents = $state<Agent[]>([]);
	let isNewAgentModalOpen = $state(false);
	let newAgentName = $state('');
	let isCreating = $state(false);

	// Initial setup / login state
	let loginServerUrl = $state('http://localhost:8000');
	let loginUsername = $state('root');
	let loginPassword = $state('');
	let isLoggingIn = $state(false);
	let loginError = $state<string | null>(null);

	async function handleInitialLogin() {
		if (!loginUsername.trim() || !loginServerUrl.trim() || !loginPassword.trim()) return;
		isLoggingIn = true;
		loginError = null;
		try {
			api.setConfig(loginServerUrl.trim(), '');
			const res = await api.auth.login(loginUsername.trim(), loginPassword.trim());
			const newAcc: ServerAccount = {
				id: res.account_id,
				serverUrl: loginServerUrl.trim(),
				username: res.username,
				token: res.token,
				role: res.role ?? 'member',
				displayName: res.username,
				avatarColor: '#3b82f6'
			};
			await authState.addAndSelectAccount(newAcc);
			loginPassword = '';
		} catch (e: any) {
			loginError = e?.message || 'Login failed. Please verify your credentials.';
		} finally {
			isLoggingIn = false;
		}
	}

	// Load agents whenever authentication is active.
	// Depends on WHICH account is active, not just `isAuthenticated`:
	// switching between two stored server profiles keeps that flag true, so a
	// boolean-only dependency would never re-run and the sidebar would keep
	// showing the previous server's agents.
	const agentScope = $derived(
		authState.isAuthenticated
			? `${authState.activeAccount?.serverUrl ?? ''}|${authState.activeAccount?.id ?? ''}`
			: ''
	);

	$effect(() => {
		if (!agentScope) {
			agents = [];
			return;
		}
		loadAgents();
	});

	// Bumped on every load so a slow response for a previously selected server
	// cannot overwrite the list of the newly selected one.
	let agentsRequestId = 0;

	async function loadAgents() {
		const requestId = ++agentsRequestId;
		const scope = agentScope;
		try {
			const list = await api.agents.list();
			if (requestId !== agentsRequestId || scope !== agentScope) return;
			agents = list;
		} catch (e) {
			if (requestId !== agentsRequestId) return;
			console.error('Failed to load agents:', e);
			agents = [];
		}
	}

	const filteredAgents = $derived(
		agents.filter((a) => a.name.toLowerCase().includes(searchQuery.toLowerCase()))
	);

	async function toggleAgentActive(agent: Agent, event?: MouseEvent) {
		if (event) {
			event.preventDefault();
			event.stopPropagation();
		}
		try {
			const updated = await api.agents.patch(agent.id, { is_active: !agent.is_active });
			agents = agents.map((a) => (a.id === agent.id ? updated : a));
		} catch (e) {
			console.error('Failed to toggle agent:', e);
		}
	}

	async function deleteAgent(agent: Agent, event?: MouseEvent) {
		if (event) {
			event.preventDefault();
			event.stopPropagation();
		}
		if (!confirm(`Are you sure you want to permanently delete agent "${agent.name}"? This action cannot be undone.`)) {
			return;
		}
		try {
			await api.agents.delete(agent.id);
			agents = agents.filter((a) => a.id !== agent.id);
			const currentAgentId = page.url.searchParams.get('agent_id');
			if (currentAgentId === agent.id) {
				window.location.href = '/';
			}
		} catch (e: any) {
			alert('Failed to delete agent: ' + (e?.message || e));
		}
	}

	async function createAgent() {
		const nameToUse = newAgentName.trim();
		if (!nameToUse) return;
		isCreating = true;
		try {
			const created = await api.agents.create({
				name: nameToUse,
				strategy: 'fallback',
				system_prompt: 'You are {{name}}, an autonomous AI assistant.\nAvailable tools: {{tools-preset}}'
			});
			agents = [...agents, created];
			newAgentName = '';
			isNewAgentModalOpen = false;
			window.location.href = `/?agent_id=${created.id}`;
		} catch (e) {
			alert('Failed to create agent: ' + e);
		} finally {
			isCreating = false;
		}
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>NeuxDOT — Minimalist Agent Platform</title>
</svelte:head>

<div id="app-root" class="flex h-screen w-screen overflow-hidden bg-zinc-50 dark:bg-zinc-950 text-zinc-900 dark:text-zinc-100 font-sans antialiased selection:bg-indigo-500 selection:text-white transition-colors duration-150">
<!-- 🧱 LEFT SIDEBAR (Navigation) -->
	<aside
 id="app-sidebar"
 class="h-full w-full shrink-0 flex-col border-r border-zinc-200 bg-zinc-100/60 backdrop-blur-xl dark:border-zinc-800 dark:bg-zinc-900/60 lg:flex lg:w-72 {sidebarOpen ? 'flex' : 'hidden'}"
 >
		<!-- Brand & Header -->
		<div id="sidebar-header" class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 px-4 py-3.5">
			<a href="/" class="flex items-center gap-2.5 group">
				<div class="flex h-7 w-7 items-center justify-center rounded-lg bg-zinc-900 dark:bg-white text-white dark:text-zinc-950 font-bold text-xs tracking-tight shadow-sm transition-transform group-hover:scale-105">
					ND
				</div>
				<div class="flex items-baseline gap-1.5">
					<span class="font-semibold text-sm tracking-tight text-zinc-900 dark:text-zinc-100">NeuxDOT</span>
					<span class="text-[10px] font-mono text-zinc-400 dark:text-zinc-500">v0.1</span>
				</div>
			</a>

			<button
type="button"
class="ml-auto mr-1 flex h-7 w-7 items-center justify-center rounded-full text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 transition-colors lg:hidden"
onclick={() => navState.hideLevel1()}
aria-label="Close navigation"
>
<X class="h-4 w-4" />
</button>

<!-- Theme Toggle Button -->
			<button
				type="button"
				class="flex h-7 w-7 items-center justify-center rounded-full text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 transition-colors"
				onclick={() => themeStore.toggle()}
				title={themeStore.isDark ? 'Switch to Bright Mode' : 'Switch to Dark Mode'}
				aria-label="Toggle bright / dark mode"
			>
				{#if themeStore.isDark}
					<Sun class="h-4 w-4" />
				{:else}
					<Moon class="h-4 w-4" />
				{/if}
			</button>
		</div>

		<!-- Search & Add Agent Button -->
		<div id="sidebar-actions" class="p-3 space-y-2 border-b border-zinc-200/80 dark:border-zinc-800/80">
			<div class="relative">
				<Search class="absolute left-2.5 top-2.5 h-3.5 w-3.5 text-zinc-400 dark:text-zinc-500" />
				<input
					type="text"
					bind:value={searchQuery}
					placeholder="Search agents..."
					class="w-full rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-950/80 py-1.5 pl-8 pr-3 text-xs text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 dark:placeholder-zinc-500 focus:border-zinc-400 dark:focus:border-zinc-600 focus:outline-none transition-colors"
				/>
			</div>

			<button
				type="button"
				class="flex w-full items-center justify-center gap-1.5 rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs active:scale-[0.99] transition-all"
				onclick={() => (isNewAgentModalOpen = true)}
			>
				<Plus class="h-3.5 w-3.5" />
				<span>New Agent</span>
			</button>
		</div>

		<!-- Middle (Agent List) -->
		<div id="sidebar-agents-section" class="flex-1 overflow-y-auto p-2 space-y-1">
			{#if filteredAgents.length === 0}
				<div class="py-12 px-4 text-center">
					<p class="text-xs font-medium text-zinc-600 dark:text-zinc-400">
						{searchQuery ? 'No matching agents' : 'No agents yet'}
					</p>
					<p class="text-[11px] text-zinc-400 dark:text-zinc-500 mt-0.5">
						Create an agent to get started
					</p>
				</div>
			{:else}
				{#each filteredAgents as agent (agent.id)}
					{@const currentAgentId = page.url.searchParams.get('agent_id')}
					{@const isSelected = currentAgentId === agent.id}
					<div
						class="group flex items-center justify-between rounded-lg px-2.5 py-2 text-xs transition-all {isSelected
							? 'bg-white dark:bg-zinc-800 text-zinc-900 dark:text-white font-medium shadow-xs border border-zinc-200/80 dark:border-zinc-700/80'
							: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/50 dark:hover:bg-zinc-800/40 border border-transparent'}"
					>
						<a
							href="/?agent_id={agent.id}"
							onclick={() => navState.hideLevel1()}
							class="flex-1 min-w-0 flex items-center gap-2 mr-2"
							title={agent.name}
						>
							<Avatar
								src={agent.avatar_url}
								name={agent.name}
								color={agent.avatar_color || '#6366f1'}
								shape={agent.avatar_shape === 'circle'
									? 'circle'
									: agent.avatar_shape === 'square'
										? 'square'
										: 'rounded'}
								size="xs"
								class="shrink-0"
							/>
							<span class="truncate font-medium">{agent.name}</span>
{#if agent.is_owner === false}
<span class="shrink-0 rounded-full border border-amber-500/20 bg-amber-500/10 px-1 py-0.5 text-[9px] font-mono text-amber-600 dark:text-amber-400" title="Shared with you">S</span>
{/if}
						</a>

						<div class="flex items-center gap-1">
							<!-- Delete Agent Button -->
							<button
								type="button"
								class="p-1 rounded-md text-zinc-400 hover:text-rose-500 hover:bg-rose-500/10 transition-colors cursor-pointer"
								onclick={(e) => deleteAgent(agent, e)}
								title="Delete agent"
								aria-label="Delete agent"
							>
								<Trash2 class="h-3.5 w-3.5" />
							</button>

							<!-- ON/OFF Toggle Switch -->
							<button
								type="button"
								class="relative inline-flex h-4 w-7 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {agent.is_active
									? 'bg-emerald-500'
									: 'bg-zinc-300 dark:bg-zinc-700'}"
								onclick={(e) => toggleAgentActive(agent, e)}
								title={agent.is_active ? 'Active (Click to turn OFF)' : 'Inactive (Click to turn ON)'}
								aria-label="Toggle agent active state"
							>
								<span
									class="pointer-events-none inline-block h-3 w-3 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {agent.is_active
										? 'translate-x-3'
										: 'translate-x-0'}"
								></span>
							</button>
						</div>
					</div>
				{/each}
			{/if}
		</div>

		<!-- Fixed Bottom Navigation -->
		<nav id="sidebar-navigation" class="border-t border-zinc-200/80 dark:border-zinc-800/80 p-2 bg-zinc-100/90 dark:bg-zinc-900/90 space-y-0.5">
			<a
				href="/providers"
				onclick={() => navState.hideLevel1()}
				class="flex items-center gap-2.5 rounded-lg px-3 py-1.5 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-zinc-100 transition-colors {page.url.pathname === '/providers' ? 'bg-zinc-200/80 dark:bg-zinc-800 text-zinc-900 dark:text-white font-medium' : ''}"
			>
				<Cpu class="h-4 w-4 text-indigo-500 dark:text-indigo-400" />
				<span>Providers</span>
			</a>
			<a
				href="/mcp"
				onclick={() => navState.hideLevel1()}
				class="flex items-center gap-2.5 rounded-lg px-3 py-1.5 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-zinc-100 transition-colors {page.url.pathname === '/mcp' ? 'bg-zinc-200/80 dark:bg-zinc-800 text-zinc-900 dark:text-white font-medium' : ''}"
			>
				<Radio class="h-4 w-4 text-amber-500 dark:text-amber-400" />
				<span>Plugins</span>
			</a>
			<a
				href="/settings"
				onclick={() => navState.hideLevel1()}
				class="flex items-center gap-2.5 rounded-lg px-3 py-1.5 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-zinc-100 transition-colors {page.url.pathname.startsWith('/settings') ? 'bg-zinc-200/80 dark:bg-zinc-800 text-zinc-900 dark:text-white font-medium' : ''}"
			>
				<Settings class="h-4 w-4 text-cyan-500 dark:text-cyan-400" />
				<span>Settings</span>
			</a>

			<!-- Profile Pill -->
			{#if authState.activeAccount}
				<a
					id="sidebar-account-profile"
					href="/settings/profile"
					onclick={() => navState.hideLevel1()}
					class="mt-2 flex items-center justify-between gap-2 rounded-xl border-t border-zinc-200/70 dark:border-zinc-800/80 pt-2 px-1 pb-1 text-[11px] text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 transition-colors"
					title="Manage your server profile"
				>
					<div class="flex items-center gap-2 truncate min-w-0">
						<Avatar
							src={authState.avatarUrl}
							name={authState.displayName}
							color={authState.avatarColor}
							shape={authState.avatarShape === 'circle' ? 'circle' : authState.avatarShape === 'square' ? 'square' : 'rounded'}
							size="xs"
						/>
						<div class="flex flex-col truncate leading-tight">
							<span class="truncate font-medium text-zinc-700 dark:text-zinc-300">{authState.displayName}</span>
							<span class="text-[10px] uppercase tracking-wider text-zinc-400 dark:text-zinc-500 font-mono">
								{authState.role}
							</span>
						</div>
					</div>
					<span class="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono truncate max-w-[80px]">
						{authState.activeAccount.serverUrl.replace(/^https?:\/\//, '')}
					</span>
				</a>
			{/if}
		</nav>
	</aside>

	<!-- 🖥️ MAIN CONTENT (Right Canvas) -->
	<main
		id="app-main"
		class="flex-1 flex-col overflow-hidden bg-zinc-50 dark:bg-zinc-950 transition-colors duration-150 lg:flex {sidebarOpen ? 'hidden' : 'flex'}"
	>
		{@render children()}
	</main>
</div>

<!-- Modal: New Agent -->
{#if isNewAgentModalOpen}
	<div id="modal-new-agent" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4">
		<div class="w-full max-w-sm rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl transition-all">
			<h3 class="text-base font-semibold text-zinc-900 dark:text-white tracking-tight">Create New Agent</h3>
			<p class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">Enter a name for your autonomous agent.</p>

			<div class="mt-4">
				<label for="agent-name-input" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300">Agent Name</label>
				<input
					id="agent-name-input"
					type="text"
					bind:value={newAgentName}
					placeholder="e.g. Coder, Analyst, Reviewer"
					class="mt-1 w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3.5 py-2 text-sm text-zinc-900 dark:text-white placeholder-zinc-400 dark:placeholder-zinc-500 focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none transition-all"
					onkeydown={(e) => e.key === 'Enter' && createAgent()}
				/>
			</div>

			<div class="mt-6 flex justify-end gap-2">
				<button
					type="button"
					class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-900 dark:hover:text-white transition-colors"
					onclick={() => (isNewAgentModalOpen = false)}
				>
					Cancel
				</button>
				<button
					type="button"
					class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
					onclick={createAgent}
					disabled={!newAgentName.trim() || isCreating}
				>
					{isCreating ? 'Creating...' : 'Create Agent'}
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Initial Account Setup / Login Modal (Shown if unauthenticated) -->
{#if !authState.isLoading && !authState.isAuthenticated}
	<div id="modal-initial-login" class="fixed inset-0 z-50 flex items-center justify-center bg-zinc-950/80 backdrop-blur-md p-4">
		<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
			<div class="flex items-center gap-3 mb-4">
				<div class="flex h-10 w-10 items-center justify-center rounded-xl bg-zinc-900 dark:bg-white text-white dark:text-zinc-950 font-bold text-sm shadow-sm">
					ND
				</div>
				<div>
					<h2 class="text-base font-semibold text-zinc-900 dark:text-white tracking-tight">Configure NeuxDOT Account</h2>
					<p class="text-xs text-zinc-500 dark:text-zinc-400">Log in with your server credentials to get started.</p>
				</div>
			</div>

			<form
				onsubmit={(e) => {
					e.preventDefault();
					handleInitialLogin();
				}}
				class="space-y-4"
			>
				<div>
					<label for="initial-server-url" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Server URL</label>
					<input
						id="initial-server-url"
						type="text"
						bind:value={loginServerUrl}
						placeholder="http://localhost:8000"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						required
					/>
				</div>

				<div>
					<label for="initial-username" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Username</label>
					<input
						id="initial-username"
						type="text"
						bind:value={loginUsername}
						placeholder="root"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						required
					/>
				</div>

				<div>
					<label for="initial-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Password</label>
					<input
						id="initial-password"
						type="password"
						bind:value={loginPassword}
						placeholder="Enter root password from server terminal"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						required
					/>
				</div>

				<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950/60 p-3 text-[11px] text-zinc-600 dark:text-zinc-400">
					🔑 The server CLI displays the <strong>root</strong> user and random password once per run. Copy and paste it here.
				</div>

				{#if loginError}
					<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-3 text-xs text-rose-600 dark:text-rose-400">
						{loginError}
					</div>
				{/if}

				<div class="pt-2">
					<button
						type="submit"
						class="w-full rounded-xl bg-zinc-900 dark:bg-white py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
						disabled={!loginUsername.trim() || !loginServerUrl.trim() || !loginPassword.trim() || isLoggingIn}
					>
						{isLoggingIn ? 'Connecting...' : 'Log In & Continue'}
					</button>
				</div>
			</form>
		</div>
	</div>
{/if}
