<script lang="ts">
	import {
		api,
		type McpServer,
		type Agent,
		type PluginPermissions,
		type CustomToolDef
	} from '$lib/api';
	import { parseCurlCommand, type ParsedCurlTool } from '$lib/utils/curlParser';
	import { navState } from '$lib/stores/nav.svelte';
	import {
		denyAll,
		permissionsForServer,
		permissionsFromManifest,
		requiredAccessList,
		resolvedPermissions
	} from '$lib/utils/permissions';
	import {
		Radio,
		Plus,
		Trash2,
		Edit3,
		X,
		ArrowLeft,
		Bell,
		Send,
		Check,
		Key,
		Shield,
		Upload,
		Database,
		Layers,
		Binary,
		Terminal,
		Globe,
		Code2,
		Play,
		RefreshCw,
		Wrench,
		ShieldCheck
	} from 'lucide-svelte';

	let servers = $state<McpServer[]>([]);
	let agents = $state<Agent[]>([]);
	let isLoading = $state(true);

	let isModalOpen = $state(false);
	let editingServer = $state<McpServer | null>(null);

	let formName = $state('');
	let formAlias = $state('');
	let formTransport = $state<'stdio' | 'binary' | 'http' | 'custom_tools'>('binary');
	let formUrlOrCommand = $state('');
	let formHeadersOrEnv = $state('');
	let formPermissions = $state<PluginPermissions>(permissionsFromManifest(null, 'binary'));
	/**
	 * Network allow-list, comma/whitespace separated. Empty = no network at all
	 * (the sandbox default); `*` = unrestricted. Binary/stdio plugins have no
	 * manifest to declare this, so the operator must be able to set it here —
	 * it is written to `permissions_json`, which `get_permissions` treats as the
	 * authoritative consent decision.
	 */
	let formNetworkAllow = $state('');

	/**
	 * What the plugin declares it needs. Derived for display only — users cannot
	 * toggle grants here; the manifest is the single source of truth.
	 */
	let formIsActive = $state(true);
	let isSaving = $state(false);

	// Custom cURL Tools State inside Add/Edit Plugin modal
	interface CustomToolItem {
		id: string;
		rawCurl: string;
		parsed: ParsedCurlTool;
		testArgs: Record<string, string>;
		testResult: any | null;
		isTesting: boolean;
	}
	let customTools = $state<CustomToolItem[]>([]);
	let activeToolIndex = $state<number>(0);
	let sessionVariables = $state<Record<string, string>>({});

	function addCustomTool(defaultCurl?: string) {
		const idx = customTools.length;
		const curl = defaultCurl || `curl -s -X GET "https://api.example.com/items/{{item_id}}" \\\n  -H "Content-Type: application/json"`;
		const parsed = parseCurlCommand(curl, idx);
		const testArgs: Record<string, string> = {};
		for (const v of parsed.variables) {
			testArgs[v.name] = '';
		}
		customTools = [
			...customTools,
			{
				id: 'tool_' + Math.random().toString(36).substring(2, 9),
				rawCurl: curl,
				parsed,
				testArgs,
				testResult: null,
				isTesting: false
			}
		];
		activeToolIndex = customTools.length - 1;
	}

	function removeCustomTool(idx: number) {
		if (customTools.length <= 1) return;
		customTools = customTools.filter((_, i) => i !== idx);
		if (activeToolIndex >= customTools.length) {
			activeToolIndex = Math.max(0, customTools.length - 1);
		}
	}

	function onCurlInput(idx: number, text: string) {
		const item = customTools[idx];
		if (!item) return;
		item.rawCurl = text;
		const parsed = parseCurlCommand(text, idx);
		item.parsed = parsed;
		for (const v of parsed.variables) {
			if (!(v.name in item.testArgs)) {
				item.testArgs[v.name] = '';
			}
		}
	}

	async function testRunTool(item: CustomToolItem) {
		item.isTesting = true;
		try {
			const res = await api.plugins.testCustomTool({
				tool: item.parsed,
				arguments: item.testArgs,
				session_variables: sessionVariables
			});
			item.testResult = res;
			if (res.updated_session_variables) {
				sessionVariables = { ...sessionVariables, ...res.updated_session_variables };
			}
		} catch (e: any) {
			item.testResult = {
				success: false,
				status: 500,
				body: e?.message || String(e),
				updated_session_variables: sessionVariables,
				error: e?.message || String(e)
			};
		} finally {
			item.isTesting = false;
		}
	}

	// Binary Upload state inside Add Plugin modal
	let isUploadingBinary = $state(false);
	let uploadedBinaryInfo = $state<{
		path: string;
		filename: string;
		size: number;
		suggested_name: string;
		suggested_alias: string;
	} | null>(null);

	/** Plugin whose required-access drawer is open. */
	let accessPlugin = $state<McpServer | null>(null);

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
			console.error('Failed to load plugins:', e);
		} finally {
			isLoading = false;
		}
	}

/**
 * Declared grants for a manifest, resolved by `$lib/utils/permissions` so the
 * canonical shape is defined in exactly one place and cannot drift from the
 * Rust deserializer (docs/HOST_BRIDGE.md §2).
 */
function parsePermissionsFromManifest(manifest: any, transport: string): PluginPermissions {
return permissionsFromManifest(manifest, transport);
}

/** Declared grants for a stored plugin row. */
function parsePermissions(srv: McpServer): PluginPermissions {
return permissionsForServer(srv);
}

	function parseManifestUiResources(srv: McpServer): { uri: string; name: string }[] {
		if (!srv.manifest_json) return [];
		try {
			const m = JSON.parse(srv.manifest_json);
			const res = m?.mcp?.resources || [];
			return res.filter((r: any) => r.uri?.startsWith('ui://') || r.mimeType?.includes('mcp-app'));
		} catch {
			return [];
		}
	}

	function parseCustomTools(srv: McpServer): string[] {
		if (srv.transport !== 'custom_tools' && srv.transport !== 'curl') return [];
		try {
			const m = JSON.parse(srv.url_or_command);
			return (m.tools || []).map((t: any) => t.name);
		} catch {
			return [];
		}
	}

	async function togglePluginActive(srv: McpServer) {
		const nextActive = srv.is_active === false ? true : false;
		servers = servers.map((s) => (s.id === srv.id ? { ...s, is_active: nextActive } : s));
		try {
			const updated = await api.mcp.update(srv.id, { is_active: nextActive });
			servers = servers.map((s) => (s.id === updated.id ? updated : s));
		} catch (e) {
			servers = servers.map((s) => (s.id === srv.id ? { ...s, is_active: !nextActive } : s));
			alert('Failed to update plugin active status: ' + e);
		}
	}

	function openCreateModal() {
		editingServer = null;
		formName = '';
		formAlias = '';
		formTransport = 'binary';
		formUrlOrCommand = '';
		formHeadersOrEnv = '';
		formPermissions = parsePermissionsFromManifest(null, 'binary');
		formNetworkAllow = '';
		formIsActive = true;
		uploadedBinaryInfo = null;
		isUploadingBinary = false;
		customTools = [];
		sessionVariables = {};
		addCustomTool();
		isModalOpen = true;
	}

	function openEditModal(s: McpServer) {
		editingServer = s;
		formName = s.name;
		formAlias = s.alias || '';
		formTransport =
			s.transport === 'binary'
				? 'binary'
				: s.transport === 'custom_tools' || s.transport === 'curl'
					? 'custom_tools'
					: (s.transport as 'stdio' | 'http');
		formUrlOrCommand = s.url_or_command;
		formHeadersOrEnv = s.headers_or_env || '';
		// Prefill from the *effective* grants (stored consent wins over the
		// manifest). Using the declared set here would silently erase an
		// operator's narrowing on the next save.
		const effective = resolvedPermissions(s);
		formPermissions = effective;
		formNetworkAllow = (effective.network?.allow ?? []).join(', ');
		formIsActive = s.is_active !== false;
		uploadedBinaryInfo = null;
		isUploadingBinary = false;
		customTools = [];
		sessionVariables = {};
		if (formTransport === 'custom_tools') {
			try {
				const m = JSON.parse(s.url_or_command);
				if (m.tools && Array.isArray(m.tools)) {
					customTools = m.tools.map((t: any, idx: number) => {
						const reconstructedCurl = `${t.captures?.cookies ? `${t.captures.cookies}=$(curl -s` : 'curl -s'} -X ${t.method} "${t.url}"${Object.entries(t.headers || {})
							.map(([k, v]) => ` \\\n  -H "${k}: ${v}"`)
							.join('')}${t.cookie_expression ? ` \\\n  -b "${t.cookie_expression}"` : ''}${t.captures?.cookies ? ` \\\n  -c -` : ''}${t.body ? ` \\\n  -d '${t.body}'` : ''}${t.captures?.cookies ? ')' : ''}`;
						// Stored tools are `CustomToolDef` JSON. They have no `variables` /
						// `session_variables` / `input_schema` shape, so using them directly as a
						// `ParsedCurlTool` breaks the editor (it reads `.variables.length`).
						// Re-parse the curl we just rebuilt, then restore what the parser
						// cannot infer (saved name, description and schema).
						const parsed = parseCurlCommand(reconstructedCurl, idx);
						if (typeof t.name === 'string') parsed.name = t.name;
						if (typeof t.description === 'string') parsed.description = t.description;
						if (t.input_schema) parsed.input_schema = t.input_schema;
						const testArgs: Record<string, string> = {};
						for (const v of parsed.variables) testArgs[v.name] = '';
						return {
							id: 'tool_' + Math.random().toString(36).substring(2, 9),
							rawCurl: reconstructedCurl,
							parsed,
							testArgs,
							testResult: null,
							isTesting: false
						};
					});
				}
			} catch {}
			if (customTools.length === 0) {
				addCustomTool();
			}
			activeToolIndex = 0;
		}
		isModalOpen = true;
	}


	async function handleBinaryFileSelected(event: Event) {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;

		isUploadingBinary = true;
		try {
			const res = await api.plugins.uploadBinary(file);
			uploadedBinaryInfo = res;
			formUrlOrCommand = res.path;
			if (!formName) {
				formName = res.suggested_name;
			}
			if (!formAlias) {
				formAlias = res.suggested_alias;
			}
		} catch (err: any) {
			alert('Failed to upload binary: ' + (err?.message || String(err)));
		} finally {
			isUploadingBinary = false;
			input.value = '';
		}
	}

	async function saveServer() {
		if (formTransport === 'custom_tools') {
			if (!formName.trim() || customTools.length === 0) return;
			const manifest = {
				tools: customTools.map((t) => ({
					name: t.parsed.name,
					description: t.parsed.description,
					method: t.parsed.method,
					url: t.parsed.url,
					headers: t.parsed.headers,
					body: t.parsed.body,
					cookie_expression: t.parsed.cookie_expression,
					captures: t.parsed.captures,
					input_schema: t.parsed.input_schema
				}))
			};
			formUrlOrCommand = JSON.stringify(manifest);
		} else {
			if (!formName.trim() || !formUrlOrCommand.trim()) return;
		}
		// Fold the network allow-list into the consent object. Entries are
		// comma/whitespace separated; `*` means unrestricted.
		const networkAllow = formNetworkAllow
			.split(/[\s,]+/)
			.map((h) => h.trim())
			.filter(Boolean);
		formPermissions = { ...formPermissions, network: { allow: networkAllow } };
		if (formTransport === 'binary' || formTransport === 'stdio') {
			formPermissions = { ...formPermissions, process: true };
		}

		isSaving = true;
		try {
			if (editingServer) {
				const updated = await api.mcp.update(editingServer.id, {
					name: formName.trim(),
					alias: formAlias.trim() || undefined,
					transport: formTransport,
					url_or_command: formUrlOrCommand.trim(),
					headers_or_env: formHeadersOrEnv.trim() || undefined,
					is_active: formIsActive,
					permissions: formPermissions
				});
				servers = servers.map((s) => (s.id === updated.id ? updated : s));
			} else {
				const created = await api.mcp.create({
					name: formName.trim(),
					alias: formAlias.trim() || undefined,
					transport: formTransport,
					url_or_command: formUrlOrCommand.trim(),
					headers_or_env: formHeadersOrEnv.trim() || undefined,
					is_active: formIsActive,
					permissions: formPermissions
				});
				servers = [...servers, created];
			}
			isModalOpen = false;
		} catch (e) {
			alert('Failed to save plugin: ' + e);
		} finally {
			isSaving = false;
		}
	}

	async function deleteServer(id: string) {
		if (!confirm('Are you sure you want to remove this plugin?')) return;
		try {
			await api.mcp.delete(id);
			servers = servers.filter((s) => s.id !== id);
		} catch (e) {
			alert('Failed to delete plugin: ' + e);
		}
	}

	function formatBytes(bytes: number) {
		if (!bytes || bytes === 0) return '0 B';
		const k = 1024;
		const sizes = ['B', 'KB', 'MB', 'GB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}
</script>

<div id="mcp-layout" class="flex h-full flex-col overflow-hidden">
	<!-- Header -->
	<header id="mcp-header" class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5">
		<div class="flex items-center gap-3">
			<a href="/" onclick={() => navState.showLevel1()} class="text-zinc-400 hover:text-zinc-800 dark:hover:text-white transition-colors">
				<ArrowLeft class="h-4 w-4" />
			</a>
			<div class="flex items-center gap-2">
				<Radio class="h-4 w-4 text-indigo-500" />
				<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">
					Plugins
				</h1>
			</div>
		</div>

		<button
			type="button"
			class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98"
			onclick={openCreateModal}
		>
			<Plus class="h-3.5 w-3.5" />
			<span>Add Plugin</span>
		</button>
	</header>

	<!-- Table Area -->
	<main id="mcp-content" class="flex-1 overflow-y-auto p-6 space-y-6">
		{#if isLoading}
			<div class="py-12 text-center text-xs text-zinc-400">Loading plugins...</div>
		{:else if servers.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<div class="flex h-12 w-12 items-center justify-center rounded-2xl bg-zinc-100 dark:bg-zinc-800 text-zinc-400 mb-3">
					<Radio class="h-6 w-6" />
				</div>
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">No Plugins Installed</h3>
				<p class="mt-1 max-w-sm text-xs text-zinc-500 dark:text-zinc-400">
					Install a native binary MCP server, attach a local stdio process, or connect a remote Streamable HTTP MCP server.
				</p>
				<button
					type="button"
					class="mt-4 rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 shadow-xs"
					onclick={openCreateModal}
				>
					Add First Plugin
				</button>
			</div>
		{:else}
			<section id="mcp-table-section" class="overflow-x-auto rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 shadow-xs">
				<table class="w-full text-left text-xs">
					<thead class="border-b border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-500 dark:text-zinc-400 font-medium">
						<tr>
							<th class="p-2.5 pl-3 md:p-3 md:pl-4">Plugin &amp; UI Hooks</th>
							<th class="p-2.5 md:p-3">Status</th>
							<th class="hidden p-3 md:table-cell">Runtime</th>
							<th class="hidden p-3 md:table-cell">Source / Target</th>
							<th class="p-2.5 pr-3 md:p-3 md:pr-4 text-right">Actions</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-zinc-200/60 dark:divide-zinc-800/60 text-zinc-700 dark:text-zinc-300">
						{#each servers as srv (srv.id)}
							{@const perms = parsePermissions(srv)}
							{@const uiResources = parseManifestUiResources(srv)}
							{@const customToolsList = parseCustomTools(srv)}
							<tr class="hover:bg-zinc-50/70 dark:hover:bg-zinc-800/50 transition-colors {srv.is_active === false ? 'opacity-65 bg-zinc-50/40 dark:bg-zinc-900/30' : ''}">
								<td class="p-2.5 pl-3 md:p-3 md:pl-4 font-medium text-zinc-900 dark:text-white">
									<div class="flex flex-col gap-1">
										<div class="flex items-center gap-2">
											<span
												class="h-2 w-2 rounded-full transition-colors {srv.is_active === false ? 'bg-zinc-400' : 'bg-emerald-500'}"
												title={srv.is_active === false ? 'Plugin disabled' : 'Plugin active'}
											></span>
											<span>{srv.name}</span>
											{#if srv.version}
												<span class="text-[10px] font-mono text-zinc-400">v{srv.version}</span>
											{/if}
											<span class="rounded px-1.5 py-0.5 text-[10px] font-mono text-zinc-500 bg-zinc-100 dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700" title="Tool alias prefix">
												@{srv.alias || srv.name.toLowerCase().replace(/[^a-z0-9]/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, '')}
											</span>
											{#if srv.headers_or_env}
												<span
													class="inline-flex items-center gap-1 rounded-md border border-emerald-500/20 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] font-mono text-emerald-600 dark:text-emerald-400"
													title={srv.transport === 'http' ? 'Custom HTTP Headers / Auth configured' : srv.transport === 'binary' ? 'Command Arguments configured' : 'Custom Environment Variables configured'}
												>
													<Key class="h-2.5 w-2.5" />
													<span>{srv.transport === 'http' ? 'Auth' : srv.transport === 'binary' ? 'Args' : 'Env'}</span>
												</span>
											{/if}
										</div>
										{#if srv.description}
											<p class="text-[11px] font-normal text-zinc-500 dark:text-zinc-400">{srv.description}</p>
										{/if}
										{#if uiResources.length > 0}
											<div class="flex flex-wrap items-center gap-1 mt-0.5">
												{#each uiResources as res}
													<span class="inline-flex items-center gap-1 rounded border border-indigo-500/30 bg-indigo-500/10 px-1.5 py-0.5 font-mono text-[10px] text-indigo-600 dark:text-indigo-300">
														<Layers class="h-2.5 w-2.5" />
														{res.uri}
													</span>
												{/each}
											</div>
										{/if}
										{#if customToolsList.length > 0}
											<div class="flex flex-wrap items-center gap-1 mt-0.5">
												{#each customToolsList as toolName}
													<span class="inline-flex items-center gap-1 rounded border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.5 font-mono text-[10px] text-emerald-600 dark:text-emerald-300" title="Custom HTTP tool">
														<Wrench class="h-2.5 w-2.5" />
														{toolName}
													</span>
												{/each}
											</div>
										{/if}
									</div>
								</td>
								<td class="p-3 whitespace-nowrap">
									<button
										type="button"
										role="switch"
										aria-checked={srv.is_active !== false}
										onclick={() => togglePluginActive(srv)}
										class="inline-flex items-center gap-2 group cursor-pointer focus:outline-none"
										title={srv.is_active !== false ? 'Plugin is active — click to turn off' : 'Plugin is turned off — click to turn on'}
									>
										<span
											class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors duration-200 ease-in-out {srv.is_active !== false ? 'bg-emerald-500 dark:bg-emerald-600' : 'bg-zinc-300 dark:bg-zinc-700'}"
										>
											<span
												class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow-xs transition-transform duration-200 ease-in-out {srv.is_active !== false ? 'translate-x-4.5' : 'translate-x-1'}"
											></span>
										</span>
										<span class="text-[11px] font-medium {srv.is_active !== false ? 'text-emerald-600 dark:text-emerald-400' : 'text-zinc-400 dark:text-zinc-500'}">
											{srv.is_active !== false ? 'Active' : 'Off'}
										</span>
									</button>
								</td>
								<td class="hidden md:table-cell p-3">
									<div class="flex flex-col items-start gap-1">
										<span class="rounded-full px-2 py-0.5 text-[10px] font-mono uppercase {srv.transport === 'binary' ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border border-purple-500/20' : srv.transport === 'custom_tools' || srv.transport === 'curl' ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20' : srv.transport === 'stdio' ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20' : 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400 border border-cyan-500/20'}">
											{srv.transport === 'binary' ? 'BINARY' : srv.transport === 'custom_tools' || srv.transport === 'curl' ? 'CUSTOM cURL' : srv.transport === 'stdio' ? 'STDIO' : srv.transport}
										</span>
									</div>
								</td>
								<td class="hidden md:table-cell p-3 font-mono text-[11px] text-zinc-500 dark:text-zinc-400 max-w-xs truncate" title={srv.url_or_command}>
									{srv.url_or_command}
								</td>
								<td class="p-2.5 pr-3 md:p-3 md:pr-4 text-right">
									<div class="flex items-center justify-end gap-1.5">
										<button
											type="button"
											class="inline-flex items-center gap-1 rounded-lg border border-zinc-200 dark:border-zinc-700 px-2 py-1 text-[11px] font-medium text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
											onclick={() => (accessPlugin = srv)}
											title="See what access this plugin requires"
										>
											<ShieldCheck class="h-3 w-3 text-emerald-500" />
											<span>Access</span>
										</button>
										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-800 dark:hover:text-white transition-colors"
											onclick={() => openEditModal(srv)}
											title="Edit Plugin"
										>
											<Edit3 class="h-3.5 w-3.5" />
										</button>
										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
											onclick={() => deleteServer(srv.id)}
											title="Delete Plugin"
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

<!-- Required Access drawer: what this plugin is allowed to touch -->
{#if accessPlugin}
{@const accessPerms = parsePermissions(accessPlugin)}
{@const accessItems = requiredAccessList(accessPerms)}
{@const unrestrictedNetwork = (accessPerms.network?.allow ?? []).includes('*')}
{@const bypassesSandbox = unrestrictedNetwork || accessPerms.elevated}
{@const controlScopes = (Object.keys(accessPerms.control ?? {}) as Array<keyof typeof accessPerms.control>).filter((k) => accessPerms.control[k] === true)}
<section class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/60 p-5 shadow-xs space-y-4">
	<div class="flex items-center justify-between border-b border-zinc-200/60 dark:border-zinc-800 pb-3">
		<div class="flex items-center gap-2">
			<ShieldCheck class="h-4 w-4 text-emerald-500" />
			<h3 class="text-xs font-semibold text-zinc-900 dark:text-white">
				Required Access & Permissions — {accessPlugin.name}
			</h3>
			{#if accessPlugin.version}
				<span class="rounded bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 font-mono text-[10px] text-zinc-500">v{accessPlugin.version}</span>
			{/if}
		</div>
		<button
			type="button"
			class="text-xs text-zinc-400 hover:text-zinc-700 dark:hover:text-white"
			onclick={() => (accessPlugin = null)}
		>
			Close
		</button>
	</div>

	<!-- Notice explaining manifest declaration -->
	<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800/80 bg-zinc-50/60 dark:bg-zinc-950/40 p-3 text-xs text-zinc-600 dark:text-zinc-300 space-y-1">
		<div class="font-medium text-emerald-500 dark:text-emerald-400 flex items-center gap-1.5">
			<ShieldCheck class="h-3.5 w-3.5" />
			<span>Declared by Plugin Manifest</span>
		</div>
		<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
			Permissions are declared upfront by the app manifest. The app cannot exceed or change these scopes.
		</p>
	</div>

	{#if controlScopes.length > 0}
		<!-- Soft warning: this plugin can mutate the server, not just read it. -->
		<div
			role="status"
			class="rounded-xl border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-800 dark:text-amber-200 space-y-1"
		>
			<div class="font-medium flex items-center gap-1.5">
				<Shield class="h-3.5 w-3.5" />
				<span>Can control the server core</span>
			</div>
			<p class="text-[11px] text-amber-700 dark:text-amber-300">
				This plugin may call the control MCP with its own credential and will act with that account's
				authority (<span class="font-mono">root</span> credentials act as root). Scopes:
				<span class="font-mono">{controlScopes.join(', ')}</span>. Only grant this to plugins you fully trust.
			</p>
		</div>
	{/if}

	{#if bypassesSandbox}
		<!-- Soft warning: the plugin is not contained by the default sandbox. -->
		<div
			role="status"
			class="rounded-xl border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-800 dark:text-amber-200 space-y-1"
		>
			<div class="font-medium flex items-center gap-1.5">
				<Shield class="h-3.5 w-3.5" />
				<span>Bypasses the default sandbox</span>
			</div>
			<p class="text-[11px] text-amber-700 dark:text-amber-300">
				{#if accessPerms.elevated}
					This plugin is declared <span class="font-mono">elevated</span> and runs on the host with no bubblewrap sandbox, no network or filesystem isolation. Only grant this to plugins you fully trust.
				{:else}
					This plugin requests unrestricted network access (<span class="font-mono">["*"]</span>) and is therefore not contained by the default no-network sandbox. Only grant this to plugins you fully trust.
				{/if}
			</p>
		</div>
	{/if}

	{#if accessItems.length === 0}
		<p class="text-xs text-zinc-500 dark:text-zinc-400 italic">
			This plugin requests no special access. It runs in strict isolation and cannot read storage, profile data, or the network.
		</p>
	{:else}
		<ul class="grid grid-cols-1 md:grid-cols-2 gap-2.5">
			{#each accessItems as item (item.group + item.scope)}
				<li class="flex flex-col gap-1 rounded-xl border border-zinc-200/70 dark:border-zinc-800 bg-zinc-50/60 dark:bg-zinc-950/40 p-3">
					<div class="flex flex-wrap items-center gap-2">
						<span class="rounded-md border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-300">
							{item.group}
						</span>
						<span class="font-mono text-[10px] text-zinc-500 dark:text-zinc-400">{item.scope}</span>
					</div>
					<p class="text-xs text-zinc-700 dark:text-zinc-300">{item.description}</p>
				</li>
			{/each}
		</ul>
	{/if}

	<div class="flex items-center justify-end pt-3 border-t border-zinc-100 dark:border-zinc-800">
		<button
			type="button"
			class="px-4 py-1.5 rounded-lg bg-zinc-100 dark:bg-zinc-800 text-xs font-medium text-zinc-800 dark:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors"
			onclick={() => (accessPlugin = null)}
		>
			Acknowledge & Close
		</button>
	</div>
</section>
{/if}
	</main>
</div>

<!-- Modal: Add/Edit Plugin -->
{#if isModalOpen}
	<div id="modal-mcp-server" class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4">
		<div class="w-full max-w-2xl rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl max-h-[90vh] overflow-y-auto">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
					{editingServer ? `Configure Plugin: ${editingServer.name}` : 'Add Plugin'}
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
				<!-- Plugin Status Toggle -->
				<div class="flex items-center justify-between rounded-xl border border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-100/70 dark:bg-zinc-800/60">
					<div class="space-y-0.5">
						<div class="flex items-center gap-2">
							<span class="text-xs font-semibold text-zinc-900 dark:text-white">Plugin Status</span>
							<span class="rounded px-1.5 py-0.2 text-[10px] font-medium {formIsActive ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20' : 'bg-zinc-200 dark:bg-zinc-700 text-zinc-600 dark:text-zinc-400 border border-zinc-300 dark:border-zinc-600'}">
								{formIsActive ? 'Active' : 'Turned Off'}
							</span>
						</div>
						<p class="text-[11px] text-zinc-600 dark:text-zinc-400">
							{formIsActive ? 'Plugin is active and available to agents and MCP-App interfaces.' : 'Plugin is turned off. Agents will ignore its tools, and UI hooks remain inactive.'}
						</p>
					</div>
					<button
						type="button"
						role="switch"
						aria-checked={formIsActive}
						onclick={() => (formIsActive = !formIsActive)}
						class="relative inline-flex h-6 w-11 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {formIsActive ? 'bg-emerald-500' : 'bg-zinc-300 dark:bg-zinc-700'}"
						title={formIsActive ? 'Click to turn off plugin' : 'Click to turn on plugin'}
					>
						<span
							class="pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow-xs transition duration-200 ease-in-out {formIsActive ? 'translate-x-5' : 'translate-x-0'}"
						></span>
					</button>
				</div>

				{#if !editingServer}
					<div>
						<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1.5">Plugin Runtime Type</span>
						<div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
							<button
								type="button"
								onclick={() => {
									formTransport = 'binary';
									formPermissions = parsePermissionsFromManifest(null, 'binary');
									if (!uploadedBinaryInfo) {
										formUrlOrCommand = '';
									}
								}}
								class="flex flex-col items-center gap-1 rounded-xl border p-2.5 text-xs font-medium transition-all cursor-pointer {formTransport === 'binary' ? 'border-purple-500 bg-purple-500/10 text-purple-600 dark:text-purple-300' : 'border-zinc-200 dark:border-zinc-800 text-zinc-600 dark:text-zinc-400'}"
							>
								<Binary class="h-4 w-4" />
								<span>Binary</span>
							</button>
							<button
								type="button"
								onclick={() => {
									formTransport = 'stdio';
									formPermissions = parsePermissionsFromManifest(null, 'stdio');
									if (!formUrlOrCommand || formUrlOrCommand.startsWith('data/binaries/')) {
										formUrlOrCommand = 'node ./mcp-tools/server.js';
									}
								}}
								class="flex flex-col items-center gap-1 rounded-xl border p-2.5 text-xs font-medium transition-all cursor-pointer {formTransport === 'stdio' ? 'border-amber-500 bg-amber-500/10 text-amber-600 dark:text-amber-300' : 'border-zinc-200 dark:border-zinc-800 text-zinc-600 dark:text-zinc-400'}"
							>
								<Terminal class="h-4 w-4" />
								<span>Stdio Process</span>
							</button>
							<button
								type="button"
								onclick={() => {
									formTransport = 'http';
									formPermissions = parsePermissionsFromManifest(null, 'http');
								}}
								class="flex flex-col items-center gap-1 rounded-xl border p-2.5 text-xs font-medium transition-all cursor-pointer {formTransport === 'http' ? 'border-cyan-500 bg-cyan-500/10 text-cyan-600 dark:text-cyan-300' : 'border-zinc-200 dark:border-zinc-800 text-zinc-600 dark:text-zinc-400'}"
							>
								<Globe class="h-4 w-4" />
								<span>HTTP / SSE</span>
							</button>
							<button
								type="button"
								onclick={() => {
									formTransport = 'custom_tools';
									formPermissions = parsePermissionsFromManifest(null, 'custom_tools');
									if (customTools.length === 0) addCustomTool();
								}}
								class="flex flex-col items-center gap-1 rounded-xl border p-2.5 text-xs font-medium transition-all cursor-pointer {formTransport === 'custom_tools' ? 'border-emerald-500 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' : 'border-zinc-200 dark:border-zinc-800 text-zinc-600 dark:text-zinc-400'}"
							>
								<Code2 class="h-4 w-4" />
								<span>Custom (cURL)</span>
							</button>
						</div>
					</div>
				{/if}

				{#if formTransport === 'custom_tools'}
					<!-- Custom Tools (cURL / HTTP Multi-Tool Builder) -->
					<div class="space-y-4">
						<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
							<div>
								<label for="custom-plugin-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Plugin Name</label>
								<input
									id="custom-plugin-name"
									type="text"
									bind:value={formName}
									placeholder="e.g. My Custom API, Internal Portal"
									class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:outline-none focus:border-emerald-500"
								/>
							</div>
							<div>
								<label for="custom-plugin-alias" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
									Tool Call Alias Prefix
								</label>
								<input
									id="custom-plugin-alias"
									type="text"
									bind:value={formAlias}
									placeholder="e.g. portal (calls will look like portal.login)"
									class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:outline-none focus:border-emerald-500"
								/>
							</div>
						</div>

						<!-- Shared Session State Banner -->
						{#if Object.keys(sessionVariables).length > 0}
							<div class="flex flex-wrap items-center gap-2 rounded-xl border border-emerald-500/20 bg-emerald-500/5 px-3 py-2 text-xs">
								<span class="font-semibold text-emerald-600 dark:text-emerald-400 text-[11px]">Active Session State:</span>
								{#each Object.entries(sessionVariables) as [k, v]}
									<span class="rounded bg-emerald-500/10 border border-emerald-500/20 px-2 py-0.5 font-mono text-[10px] text-emerald-700 dark:text-emerald-300">
										${k} = {v.length > 28 ? v.substring(0, 28) + '...' : v}
									</span>
								{/each}
								<button
									type="button"
									onclick={() => (sessionVariables = {})}
									class="text-[10px] text-zinc-400 hover:text-rose-500 ml-auto"
								>
									Clear Session
								</button>
							</div>
						{/if}

						<!-- Tools Tabs Navigation -->
						<div class="border-b border-zinc-200 dark:border-zinc-800 pb-2">
							<div class="flex items-center justify-between mb-2">
								<span class="text-xs font-semibold uppercase tracking-wider text-zinc-400">
									Plugin Tools ({customTools.length})
								</span>
								<button
									type="button"
									onclick={() => addCustomTool()}
									class="flex items-center gap-1 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-medium text-emerald-600 dark:text-emerald-300 hover:bg-emerald-500/20"
								>
									<Plus class="h-3 w-3" />
									<span>Add Another Tool</span>
								</button>
							</div>

							<div class="flex flex-wrap items-center gap-1.5">
								{#each customTools as toolItem, idx}
									<div class="flex items-center rounded-xl border transition-all {activeToolIndex === idx ? 'border-emerald-500 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300' : 'border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 text-zinc-600 dark:text-zinc-400'}">
										<button
											type="button"
											onclick={() => (activeToolIndex = idx)}
											class="px-3 py-1.5 text-xs font-medium font-mono"
										>
											{toolItem.parsed.name || `tool_${idx + 1}`}
										</button>
										{#if customTools.length > 1}
											<button
												type="button"
												onclick={() => removeCustomTool(idx)}
												class="pr-2 text-zinc-400 hover:text-rose-500"
												title="Remove this tool"
											>
												<X class="h-3 w-3" />
											</button>
										{/if}
									</div>
								{/each}
							</div>
						</div>

						<!-- Selected Tool Editor -->
						{#if customTools[activeToolIndex]}
							{@const currentTool = customTools[activeToolIndex]}
							<div class="space-y-3 rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/50 p-4">
								<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
									<div>
										<label for="tool-name-{activeToolIndex}" class="block text-[11px] font-medium text-zinc-600 dark:text-zinc-400 mb-1">Tool Name (for LLM)</label>
										<input
											id="tool-name-{activeToolIndex}"
											type="text"
											bind:value={currentTool.parsed.name}
											class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-3 py-1.5 text-xs font-mono text-zinc-900 dark:text-white"
										/>
									</div>
									<div>
										<label for="tool-desc-{activeToolIndex}" class="block text-[11px] font-medium text-zinc-600 dark:text-zinc-400 mb-1">Tool Description (for LLM)</label>
										<input
											id="tool-desc-{activeToolIndex}"
											type="text"
											bind:value={currentTool.parsed.description}
											class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-3 py-1.5 text-xs text-zinc-900 dark:text-white"
										/>
									</div>
								</div>

								<div>
									<div class="flex items-center justify-between mb-1">
										<label for="tool-curl-{activeToolIndex}" class="text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
											Paste cURL Command
										</label>
										<span class="text-[10px] text-zinc-400">
											Supports <code class="text-emerald-500">COOKIES=$(curl ...)</code>, <code class="text-emerald-500">-b "$COOKIES"</code>, <code class="text-indigo-500">&#123;&#123;arg&#125;&#125;</code>
										</span>
									</div>
									<textarea
										id="tool-curl-{activeToolIndex}"
										rows="4"
										value={currentTool.rawCurl}
										oninput={(e) => onCurlInput(activeToolIndex, (e.target as HTMLTextAreaElement).value)}
										placeholder={'COOKIES=$(curl -s -c - -X POST "https://example.com/login" \\\n  -H "Content-Type: application/json" \\\n  -d \'{"username": "{{username}}", "password": "{{password}}"}\')'}
										class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 p-3 font-mono text-xs text-zinc-900 dark:text-white focus:outline-none focus:border-emerald-500"
									></textarea>
								</div>

								<!-- Parsed Specification Summary Card -->
								<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-3 space-y-2 text-xs">
									<div class="flex flex-wrap items-center gap-2">
										<span class="rounded bg-indigo-500/10 border border-indigo-500/20 px-2 py-0.5 font-mono text-[10px] font-bold text-indigo-600 dark:text-indigo-400 uppercase">
											{currentTool.parsed.method}
										</span>
										<span class="font-mono text-zinc-700 dark:text-zinc-300 break-all">{currentTool.parsed.url || 'No URL specified'}</span>
									</div>

									{#if currentTool.parsed.cookie_expression || currentTool.parsed.captures?.cookies}
										<div class="flex flex-wrap items-center gap-2 pt-1 border-t border-zinc-100 dark:border-zinc-800 text-[11px]">
											{#if currentTool.parsed.captures?.cookies}
												<span class="inline-flex items-center gap-1 rounded bg-amber-500/10 border border-amber-500/20 px-2 py-0.5 text-amber-700 dark:text-amber-300 font-mono text-[10px]">
													Captures &rarr; ${currentTool.parsed.captures.cookies}
												</span>
											{/if}
											{#if currentTool.parsed.cookie_expression}
												<span class="inline-flex items-center gap-1 rounded bg-cyan-500/10 border border-cyan-500/20 px-2 py-0.5 text-cyan-700 dark:text-cyan-300 font-mono text-[10px]">
													Sends Cookie: {currentTool.parsed.cookie_expression}
												</span>
											{/if}
										</div>
									{/if}

									<!-- Detected LLM Arguments -->
									{#if currentTool.parsed.variables?.length > 0}
										<div class="pt-2 border-t border-zinc-100 dark:border-zinc-800 space-y-1.5">
											<div class="text-[10px] font-semibold uppercase text-zinc-400">
												Agent Input Arguments (auto-exposed in tool schema)
											</div>
											<div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
												{#each currentTool.parsed.variables as v}
													<div class="flex items-center gap-2 rounded-lg border border-zinc-200 dark:border-zinc-800 p-2 bg-zinc-50 dark:bg-zinc-950">
														<code class="text-indigo-600 dark:text-indigo-400 font-bold">{v.name}</code>
														<input
															type="text"
															bind:value={v.description}
															placeholder="Description for model..."
															class="flex-1 bg-transparent text-[11px] text-zinc-700 dark:text-zinc-300 focus:outline-none"
														/>
													</div>
												{/each}
											</div>
										</div>
									{/if}

									<!-- Live Test Runner Playground -->
									<div class="mt-3 pt-3 border-t border-zinc-100 dark:border-zinc-800 space-y-2">
										<div class="flex items-center justify-between">
											<span class="text-[11px] font-semibold text-zinc-700 dark:text-zinc-300">
												Live Test Runner
											</span>
											<button
												type="button"
												onclick={() => testRunTool(currentTool)}
												disabled={currentTool.isTesting || !currentTool.parsed.url}
												class="flex items-center gap-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 px-3 py-1 text-xs font-medium text-white shadow-xs disabled:opacity-40 transition-colors"
											>
												<Play class="h-3 w-3" />
												<span>{currentTool.isTesting ? 'Sending...' : 'Test Run'}</span>
											</button>
										</div>

										{#if currentTool.parsed.variables?.length > 0}
											<div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
												{#each currentTool.parsed.variables as v}
													<div>
														<label for="test-{v.name}" class="block text-[10px] text-zinc-500 mb-0.5">Test value for {v.name}</label>
														<input
															id="test-{v.name}"
															type="text"
															value={currentTool.testArgs[v.name] ?? ''}
											oninput={(e) => (currentTool.testArgs[v.name] = (e.currentTarget as HTMLInputElement).value)}
															placeholder={`e.g. sample_${v.name}`}
															class="w-full rounded-lg border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-2.5 py-1 text-xs font-mono text-zinc-900 dark:text-white"
														/>
													</div>
												{/each}
											</div>
										{/if}

										{#if currentTool.testResult}
											<div class="rounded-xl border p-2.5 text-xs font-mono space-y-1 {currentTool.testResult.success ? 'border-emerald-500/30 bg-emerald-500/5' : 'border-rose-500/30 bg-rose-500/5'}">
												<div class="flex items-center justify-between">
													<span class="font-bold {currentTool.testResult.success ? 'text-emerald-500' : 'text-rose-500'}">
														Status: {currentTool.testResult.status}
													</span>
													{#if currentTool.testResult.updated_session_variables?.COOKIES}
														<span class="text-[10px] text-emerald-600 dark:text-emerald-400">
															Captured $COOKIES: {currentTool.testResult.updated_session_variables.COOKIES.substring(0, 24)}...
														</span>
													{/if}
												</div>
												<pre class="max-h-32 overflow-y-auto whitespace-pre-wrap break-all text-[11px] text-zinc-700 dark:text-zinc-300 bg-white/50 dark:bg-black/30 p-2 rounded-lg">{currentTool.testResult.body}</pre>
											</div>
										{/if}
									</div>
								</div>
							</div>
						{/if}
					</div>
				{:else}
					{#if formTransport === 'binary'}
						<!-- Upload Native Binary Section -->
						<div class="space-y-3 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-800/30 p-4">
							{#if editingServer}
								<div class="rounded-xl border border-zinc-200 dark:border-zinc-700/60 bg-white dark:bg-zinc-900 p-3 text-xs space-y-1.5">
									<div class="flex items-center justify-between">
										<div class="flex items-center gap-2">
											<Binary class="h-4 w-4 text-purple-500" />
											<span class="font-semibold text-zinc-900 dark:text-zinc-100">Installed Binary</span>
											<span class="rounded bg-purple-500/10 text-purple-600 dark:text-purple-400 border border-purple-500/20 px-1.5 py-0.2 font-mono text-[10px]">v{editingServer.version || '1.0.0'}</span>
										</div>
									</div>
									<div class="text-[10px] font-mono text-zinc-400 dark:text-zinc-500 truncate" title={editingServer.url_or_command}>
										Server Path: {editingServer.url_or_command}
									</div>
								</div>
							{/if}

							<label class="flex flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed border-zinc-300 dark:border-zinc-700 p-4 text-center cursor-pointer hover:border-purple-500 transition-colors bg-white/50 dark:bg-zinc-900/50">
								<Upload class="h-5 w-5 text-purple-500" />
								<span class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
									{isUploadingBinary ? 'Uploading binary to server...' : editingServer ? 'Choose New Binary File to Replace / Upgrade' : 'Choose / Upload Native Binary File'}
								</span>
								<span class="text-[11px] text-zinc-400">Upload executable to server (saved to data/binaries/ with executable permissions)</span>
								<input type="file" onchange={handleBinaryFileSelected} class="hidden" disabled={isUploadingBinary} />
							</label>

							{#if uploadedBinaryInfo}
								<div class="rounded-xl border border-purple-500/30 bg-purple-500/10 p-3 text-xs space-y-1">
									<div class="flex items-center justify-between">
										<span class="font-semibold text-purple-300">Ready to run: {uploadedBinaryInfo.filename}</span>
										<span class="font-mono text-[10px] text-purple-400">{formatBytes(uploadedBinaryInfo.size)}</span>
									</div>
									<div class="text-[11px] font-mono text-zinc-300">Server Path: {uploadedBinaryInfo.path}</div>
								</div>
							{/if}
						</div>
					{/if}

					<div>
						<label for="mcp-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Plugin Name</label>
						<input
							id="mcp-name"
							type="text"
							bind:value={formName}
							placeholder="e.g. Git Tools, Filesystem, Database MCP"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						/>
					</div>

					<div>
						<label for="mcp-alias" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
							Unique Tool Call Alias (Optional)
						</label>
						<input
							id="mcp-alias"
							type="text"
							bind:value={formAlias}
							placeholder="e.g. telegram, discord, fs (defaults to plugin name slug)"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						/>
					</div>

					{#if formTransport !== 'binary'}
						<div>
							<div class="flex items-center justify-between mb-1">
								<label for="mcp-cmd" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300">
									{formTransport === 'stdio' ? 'Command + Arguments' : 'Streamable HTTP / SSE Endpoint URL'}
								</label>
							</div>
							<input
								id="mcp-cmd"
								type="text"
								bind:value={formUrlOrCommand}
								placeholder={formTransport === 'stdio'
									? 'node ./mcp-tools/server.js'
									: 'http://localhost:3001/sse'}
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
							/>
						</div>
					{/if}

						<div>
							<label for="mcp-headers" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
								{#if formTransport === 'http'}
									Headers & Authentication (Optional)
								{:else if formTransport === 'binary'}
									Command Arguments (Optional)
								{:else}
									Environment Variables (Optional)
								{/if}
							</label>
							<textarea
								id="mcp-headers"
								rows="3"
								bind:value={formHeadersOrEnv}
								placeholder={formTransport === 'http'
									? '{"Authorization": "Bearer YOUR_API_KEY"}\nor\nAuthorization: Bearer YOUR_API_KEY'
									: formTransport === 'binary'
									? '--token YOUR_TOKEN --read-only\nor\n["--token", "YOUR_TOKEN"]'
									: '{"GITHUB_TOKEN": "ghp_xxx"}\nor\nKEY=VALUE'}
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
							></textarea>
							{#if formTransport === 'binary'}
								<p class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
									CLI flags or arguments passed directly to the binary process upon startup.
								</p>
							{/if}
						</div>
				{/if}

				<!-- Network allow-list. The sandbox default is no network; this is the
				     only way to grant a binary/stdio plugin outbound access, because
				     such a plugin has no manifest to declare it. -->
				{#if formTransport === 'binary' || formTransport === 'stdio'}
					<div class="mt-4 rounded-xl border border-zinc-200/80 dark:border-zinc-800/80 bg-zinc-50/60 dark:bg-zinc-950/40 p-3 space-y-1.5">
						<div class="flex items-center gap-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300">
							<Globe class="h-3.5 w-3.5 text-sky-500" />
							<span>Outbound Network Allow-list</span>
						</div>
						<input
							id="mcp-network-allow"
							type="text"
							bind:value={formNetworkAllow}
							placeholder="api.telegram.org, *.example.com, 10.0.0.0/8"
							class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-950 px-2.5 py-1.5 text-xs font-mono text-zinc-900 dark:text-white focus:outline-none focus:border-sky-500"
						/>
						<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
							Leave empty for <span class="font-medium">no network</span> (the default). Comma-separated hostnames,
							{'*.suffix'} wildcards, IPs or CIDRs; <code class="font-mono">*</code> means unrestricted. Enforced by a
							host-side CONNECT proxy bound into the sandbox.
						</p>
					</div>
				{/if}

				<!-- Read-only: the sandbox default is a host policy, not a per-plugin toggle. -->
				<div class="mt-4 rounded-xl border border-zinc-200/80 dark:border-zinc-800/80 bg-zinc-50/60 dark:bg-zinc-950/40 p-3">
					<div class="flex items-center gap-1.5 text-xs font-medium text-emerald-500 dark:text-emerald-400">
						<ShieldCheck class="h-3.5 w-3.5" />
						<span>Sandboxed by Default</span>
					</div>
					<p class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
						Sandboxed: runs in an isolated bubblewrap sandbox with no host filesystem access. Outbound network is denied unless you grant it above.
					</p>
					<p class="mt-1 text-[11px] text-zinc-400 dark:text-zinc-500">
						The allow-list is saved as this plugin's consent decision; it can narrow, but never widen, what the manifest declared.
					</p>
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
						class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs cursor-pointer"
						onclick={saveServer}
						disabled={isSaving ||
							(formTransport === 'custom_tools'
								? !formName.trim() || customTools.length === 0 || !customTools[0]?.parsed?.url
								: !formName.trim() || !formUrlOrCommand.trim())}
					>
						{isSaving
							? 'Saving...'
							: editingServer
								? 'Update Plugin'
								: 'Save Plugin'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
