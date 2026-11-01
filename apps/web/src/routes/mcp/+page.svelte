<script lang="ts">
	import { api, type McpServer, type Agent } from '$lib/api';
	import { Radio, Plus, Trash2, Edit3, X, ArrowLeft, Bell, Send, Check } from 'lucide-svelte';

	let servers = $state<McpServer[]>([]);
	let agents = $state<Agent[]>([]);
	let isLoading = $state(true);

	let isModalOpen = $state(false);
	let editingServer = $state<McpServer | null>(null);

	let formName = $state('');
	let formTransport = $state('stdio');
	let formUrlOrCommand = $state('node ./mcp-tools/server.js');
	let isSaving = $state(false);

	// Push simulation modal
	let isPushModalOpen = $state(false);
	let pushServer = $state<McpServer | null>(null);
	let pushMethod = $state('notifications/message');
	let pushPayload = $state('{\n  "level": "info",\n  "data": "New external git commit detected in repo"\n}');
	let pushTargetAgentId = $state<string>('');
	let isPushing = $state(false);
	let pushSuccess = $state(false);

	$effect(() => {
		loadData();
	});

	async function loadData() {
		isLoading = true;
		try {
			const [sList, aList] = await Promise.all([api.mcp.list(), api.agents.list()]);
			servers = sList;
			agents = aList;
		} catch (e) {
			console.error('Failed to load MCP servers:', e);
		} finally {
			isLoading = false;
		}
	}

	function openCreateModal() {
		editingServer = null;
		formName = '';
		formTransport = 'stdio';
		formUrlOrCommand = 'node ./mcp-tools/server.js';
		isModalOpen = true;
	}

	function openEditModal(s: McpServer) {
		editingServer = s;
		formName = s.name;
		formTransport = s.transport;
		formUrlOrCommand = s.url_or_command;
		isModalOpen = true;
	}

	function openPushModal(s: McpServer) {
		pushServer = s;
		pushMethod = 'notifications/message';
		pushPayload = '{\n  "level": "info",\n  "data": "Server-pushed notification update"\n}';
		pushTargetAgentId = agents[0]?.id || '';
		pushSuccess = false;
		isPushModalOpen = true;
	}

	async function saveServer() {
		if (!formName.trim() || !formUrlOrCommand.trim()) return;
		isSaving = true;
		try {
			if (editingServer) {
				const updated = await api.mcp.update(editingServer.id, {
					name: formName.trim(),
					transport: formTransport,
					url_or_command: formUrlOrCommand.trim()
				});
				servers = servers.map((s) => (s.id === updated.id ? updated : s));
			} else {
				const created = await api.mcp.create({
					name: formName.trim(),
					transport: formTransport,
					url_or_command: formUrlOrCommand.trim()
				});
				servers = [...servers, created];
			}
			isModalOpen = false;
		} catch (e) {
			alert('Failed to save MCP server: ' + e);
		} finally {
			isSaving = false;
		}
	}

	async function deleteServer(id: string) {
		if (!confirm('Are you sure you want to remove this MCP server?')) return;
		try {
			await api.mcp.delete(id);
			servers = servers.filter((s) => s.id !== id);
		} catch (e) {
			alert('Failed to delete MCP server: ' + e);
		}
	}

	async function executePush() {
		if (!pushServer) return;
		isPushing = true;
		try {
			let parsed: unknown = null;
			try {
				parsed = JSON.parse(pushPayload);
			} catch {
				parsed = { raw: pushPayload };
			}

			await api.mcp.push(
				pushServer.id,
				pushMethod.trim(),
				parsed,
				pushTargetAgentId || undefined
			);

			pushSuccess = true;
			setTimeout(() => {
				isPushModalOpen = false;
				pushSuccess = false;
			}, 1200);
		} catch (e) {
			alert('Failed to push information to agent: ' + e);
		} finally {
			isPushing = false;
		}
	}
</script>

<div id="mcp-layout" class="flex h-full flex-col overflow-hidden">
	<!-- Header -->
	<header id="mcp-header" class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5">
		<div class="flex items-center gap-3">
			<a href="/" class="text-zinc-400 hover:text-zinc-800 dark:hover:text-white transition-colors">
				<ArrowLeft class="h-4 w-4" />
			</a>
			<div class="flex items-center gap-2">
				<Radio class="h-4 w-4 text-amber-500" />
				<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">External MCP Servers</h1>
			</div>
		</div>

		<button
			type="button"
			class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98"
			onclick={openCreateModal}
		>
			<Plus class="h-3.5 w-3.5" />
			<span>Add MCP Server</span>
		</button>
	</header>

	<!-- Table Area -->
	<main id="mcp-content" class="flex-1 overflow-y-auto p-6">
		{#if isLoading}
			<div class="py-12 text-center text-xs text-zinc-400">Loading MCP servers...</div>
		{:else if servers.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<div class="flex h-12 w-12 items-center justify-center rounded-2xl bg-zinc-100 dark:bg-zinc-800 text-zinc-400 mb-3">
					<Radio class="h-6 w-6" />
				</div>
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">No MCP Servers Connected</h3>
				<p class="mt-1 max-w-sm text-xs text-zinc-500 dark:text-zinc-400">
					NeuxDOT agent core has zero built-in tools. Add an external MCP server (stdio command or streamable HTTP).
				</p>
				<button
					type="button"
					class="mt-4 rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 shadow-xs"
					onclick={openCreateModal}
				>
					Add First Server
				</button>
			</div>
		{:else}
			<section id="mcp-table-section" class="overflow-hidden rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 shadow-xs">
				<table class="w-full text-left text-xs">
					<thead class="border-b border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-500 dark:text-zinc-400 font-medium">
						<tr>
							<th class="p-3 pl-4">Server Name</th>
							<th class="p-3">Transport</th>
							<th class="p-3">Command or Stream URL</th>
							<th class="p-3 pr-4 text-right">Actions</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-zinc-200/60 dark:divide-zinc-800/60 text-zinc-700 dark:text-zinc-300">
						{#each servers as srv (srv.id)}
							<tr class="hover:bg-zinc-50/70 dark:hover:bg-zinc-800/50 transition-colors">
								<td class="p-3 pl-4 font-medium text-zinc-900 dark:text-white">{srv.name}</td>
								<td class="p-3">
									<span class="rounded-full px-2 py-0.5 text-[10px] font-mono uppercase {srv.transport === 'wasm_stdio' ? 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20' : srv.transport === 'stdio' ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20' : 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400 border border-cyan-500/20'}">
										{srv.transport}
									</span>
								</td>
								<td class="p-3 font-mono text-zinc-500 dark:text-zinc-400">{srv.url_or_command}</td>
								<td class="p-3 pr-4 text-right">
									<div class="flex items-center justify-end gap-1.5">
										<!-- Push Info Action -->
										<button
											type="button"
											class="flex items-center gap-1 rounded-lg bg-zinc-100 dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 px-2 py-1 text-[11px] font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors shadow-2xs"
											onclick={() => openPushModal(srv)}
											title="Push notification to agent"
										>
											<Bell class="h-3 w-3 text-amber-500" />
											<span>Push Info</span>
										</button>

										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-800 dark:hover:text-white transition-colors"
											onclick={() => openEditModal(srv)}
											title="Edit"
										>
											<Edit3 class="h-3.5 w-3.5" />
										</button>
										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
											onclick={() => deleteServer(srv.id)}
											title="Delete"
										>
											<Trash2 class="h-3.5 w-3.5" />
										</button>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}
	</main>
</div>

<!-- Modal: Add/Edit Server -->
{#if isModalOpen}
	<div id="modal-mcp-server" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4">
		<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
					{editingServer ? 'Edit MCP Server' : 'Add MCP Server'}
				</h3>
				<button
					type="button"
					class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
					onclick={() => (isModalOpen = false)}
				>
					<X class="h-4 w-4" />
				</button>
			</div>

			<div class="mt-4 space-y-4">
				<div>
					<label for="mcp-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Server Name</label>
					<input
						id="mcp-name"
						type="text"
						bind:value={formName}
						placeholder="e.g. Git Tools, Filesystem, Database MCP"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					/>
				</div>

				<div>
					<label for="mcp-transport" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Transport</label>
					<select
						id="mcp-transport"
						bind:value={formTransport}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					>
						<option value="stdio">stdio (Local Command)</option>
						<option value="http">http / SSE (Streamable URL)</option>
					</select>
				</div>

				<div>
					<label for="mcp-cmd" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
						{formTransport === 'stdio' ? 'Command + Arguments' : 'SSE Endpoint URL'}
					</label>
					<input
						id="mcp-cmd"
						type="text"
						bind:value={formUrlOrCommand}
						placeholder={formTransport === 'stdio'
							? 'npx -y @modelcontextprotocol/server-filesystem /path'
							: 'http://localhost:3001/sse'}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					/>
				</div>

				<div class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3">
					<button
						type="button"
						class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
						onclick={() => (isModalOpen = false)}
					>
						Cancel
					</button>
					<button
						type="button"
						class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
						onclick={saveServer}
						disabled={!formName.trim() || !formUrlOrCommand.trim() || isSaving}
					>
						{isSaving ? 'Saving...' : editingServer ? 'Update Server' : 'Add Server'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}

<!-- Modal: Push Information to Agent -->
{#if isPushModalOpen && pushServer}
	<div id="modal-mcp-push" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4">
		<div class="w-full max-w-lg rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<div class="flex items-center gap-2">
					<Bell class="h-4 w-4 text-amber-500" />
					<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
						Push Notification ({pushServer.name})
					</h3>
				</div>
				<button
					type="button"
					class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
					onclick={() => (isPushModalOpen = false)}
				>
					<X class="h-4 w-4" />
				</button>
			</div>

			<div class="mt-4 space-y-4">
				<p class="text-xs text-zinc-500 dark:text-zinc-400">
					Simulate an unsolicited push notification sent from this MCP server. If an agent is running or sleeping, it will receive this information immediately.
				</p>

				<div>
					<label for="push-method" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Method / Topic</label>
					<input
						id="push-method"
						type="text"
						bind:value={pushMethod}
						placeholder="notifications/message"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					/>
				</div>

				<div>
					<label for="push-agent" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Target Agent</label>
					<select
						id="push-agent"
						bind:value={pushTargetAgentId}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					>
						<option value="">Broadcast to All Connected Agents</option>
						{#each agents as a}
							<option value={a.id}>{a.name}</option>
						{/each}
					</select>
				</div>

				<div>
					<label for="push-payload" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Payload JSON</label>
					<textarea
						id="push-payload"
						rows="4"
						bind:value={pushPayload}
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 p-2.5 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
					></textarea>
				</div>

				<div class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3">
					<button
						type="button"
						class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
						onclick={() => (isPushModalOpen = false)}
					>
						Cancel
					</button>
					<button
						type="button"
						class="flex items-center gap-1.5 rounded-xl bg-amber-500 hover:bg-amber-600 px-4 py-2 text-xs font-medium text-white disabled:opacity-40 transition-all shadow-xs"
						onclick={executePush}
						disabled={isPushing}
					>
						{#if pushSuccess}
							<Check class="h-3.5 w-3.5" />
							<span>Pushed!</span>
						{:else}
							<Send class="h-3.5 w-3.5" />
							<span>{isPushing ? 'Pushing...' : 'Send Push Alert'}</span>
						{/if}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
