<script lang="ts">
	import {
		api,
		type McpServer,
		type Agent,
		type PluginPermissions,
		type ExtensionKvEntry,
		type ExtensionStorageInfo,
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
		Terminal,
		Globe,
		Code2,
		Play,
		RefreshCw,
		Wrench
	} from 'lucide-svelte';

	let servers = $state<McpServer[]>([]);
	let agents = $state<Agent[]>([]);
	let isLoading = $state(true);

	let isModalOpen = $state(false);
	let editingServer = $state<McpServer | null>(null);

	let formName = $state('');
	let formAlias = $state('');
	let formTransport = $state<'stdio' | 'http' | 'wasm' | 'custom_tools'>('stdio');
	let formUrlOrCommand = $state('node ./mcp-tools/server.js');
	let formHeadersOrEnv = $state('');
	let formPermissions = $state<PluginPermissions>(permissionsFromManifest(null, 'stdio'));

	/**
	 * What the plugin declares it needs. Derived for display only — users cannot
	 * toggle grants here; the manifest is the single source of truth.
	 */
	const formRequiredAccess = $derived(requiredAccessList(formPermissions));
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

	// WASM Upload / URL installation state inside Add Plugin modal
	let wasmUrl = $state('');
	let wasmInspectPending = $state<{
		file: File;
		id: string;
		name: string;
		version: string;
		description?: string;
		wasm_hash: string;
		manifest: any;
		existing: any | null;
	} | null>(null);
	let isInspectingWasm = $state(false);

	// Storage / KV Inspector drawer for WASM plugins
	let selectedStoragePlugin = $state<McpServer | null>(null);
	let storageInfo = $state<ExtensionStorageInfo | null>(null);
	let kvEntries = $state<ExtensionKvEntry[]>([]);
	let loadingStorage = $state(false);

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
		formTransport = 'stdio';
		formUrlOrCommand = 'node ./mcp-tools/server.js';
		formHeadersOrEnv = '';
		formPermissions = parsePermissionsFromManifest(null, 'stdio');
		formIsActive = true;
		wasmInspectPending = null;
		wasmUrl = '';
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
			s.transport === 'wasm_stdio'
				? 'wasm'
				: s.transport === 'custom_tools' || s.transport === 'curl'
					? 'custom_tools'
					: (s.transport as 'stdio' | 'http');
		formUrlOrCommand = s.url_or_command;
		formHeadersOrEnv = s.headers_or_env || '';
		formPermissions = parsePermissions(s);
		formIsActive = s.is_active !== false;
		wasmInspectPending = null;
		customTools = [];
		sessionVariables = {};
		if (formTransport === 'custom_tools') {
			try {
				const m = JSON.parse(s.url_or_command);
				if (m.tools && Array.isArray(m.tools)) {
					customTools = m.tools.map((t: any, idx: number) => {
						const reconstructedCurl = `${t.captures?.cookies ? `${t.captures.cookies}=$(` : ''}curl -s -X ${t.method} "${t.url}"${Object.entries(t.headers || {})
							.map(([k, v]) => ` \\\n  -H "${k}: ${v}"`)
							.join('')}${t.cookie_expression ? ` \\\n  -b "${t.cookie_expression}"` : ''}${t.captures?.cookies ? ` \\\n  -c -` : ''}${t.body ? ` \\\n  -d '${t.body}'` : ''}${t.captures?.cookies ? ')' : ''}`;
						return {
							id: 'tool_' + Math.random().toString(36).substring(2, 9),
							rawCurl: reconstructedCurl,
							parsed: t,
							testArgs: {},
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


	async function handleWasmFileSelected(event: Event) {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;

		isInspectingWasm = true;
		try {
			const info = await api.plugins.inspectWasm(file);
			wasmInspectPending = { file, ...info };
			formPermissions = parsePermissionsFromManifest(info.manifest, 'wasm');
		} catch (e: any) {
			alert('Failed to inspect WASM plugin: ' + (e.message || e));
		} finally {
			isInspectingWasm = false;
		}
	}

	async function confirmInstallWasmFile() {
		if (!wasmInspectPending) return;
		isSaving = true;
		try {
			await api.plugins.uploadWasm(wasmInspectPending.file);
			await loadData();
			wasmInspectPending = null;
			isModalOpen = false;
		} catch (e: any) {
			alert('Failed to install/update WASM plugin: ' + (e.message || e));
		} finally {
			isSaving = false;
		}
	}

	async function installWasmFromUrl() {
		if (!wasmUrl.trim()) return;
		isSaving = true;
		try {
			await api.plugins.installWasmUrl(wasmUrl.trim());
			await loadData();
			wasmUrl = '';
			isModalOpen = false;
		} catch (e: any) {
			alert('Failed to install/update WASM plugin from URL: ' + (e.message || e));
		} finally {
			isSaving = false;
		}
	}

	async function saveServer() {
		// Fold the key-prefix text box back into the grants before persisting.
		if (formTransport === 'wasm') {
			if (wasmInspectPending) {
				await confirmInstallWasmFile();
				if (editingServer) {
					await api.mcp.update(editingServer.id, {
						alias: formAlias.trim() || undefined,
						is_active: formIsActive,
						permissions: formPermissions
					});
					await loadData();
				}
				return;
			} else if (wasmUrl.trim()) {
				await installWasmFromUrl();
				if (editingServer) {
					await api.mcp.update(editingServer.id, {
						alias: formAlias.trim() || undefined,
						is_active: formIsActive,
						permissions: formPermissions
					});
					await loadData();
				}
				return;
			} else if (editingServer) {
				isSaving = true;
				try {
					const updated = await api.mcp.update(editingServer.id, {
						alias: formAlias.trim() || undefined,
						is_active: formIsActive,
						permissions: formPermissions
					});
					servers = servers.map((s) => (s.id === updated.id ? updated : s));
					isModalOpen = false;
				} catch (e) {
					alert('Failed to update WASM plugin: ' + e);
				} finally {
					isSaving = false;
				}
				return;
			}
			return;
		}
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
		isSaving = true;
		try {
			if (editingServer) {
				const updated = await api.mcp.update(editingServer.id, {
					name: formName.trim(),
					alias: formAlias.trim() || undefined,
					transport: editingServer.transport === 'wasm_stdio' ? 'wasm_stdio' : formTransport,
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
			if (selectedStoragePlugin?.id === id) {
				selectedStoragePlugin = null;
			}
		} catch (e) {
			alert('Failed to delete plugin: ' + e);
		}
	}

	async function inspectWasmStorage(srv: McpServer) {
		selectedStoragePlugin = srv;
		loadingStorage = true;
		try {
			const [sInfo, kvs] = await Promise.all([
				api.plugins.getStorage(srv.id),
				api.plugins.getKv(srv.id)
			]);
			storageInfo = sInfo;
			kvEntries = kvs;
		} catch (e) {
			console.error('Failed to load plugin storage:', e);
		} finally {
			loadingStorage = false;
		}
	}

	async function deleteKvEntry(pluginId: string, key: string) {
		try {
			await api.plugins.deleteKvKey(pluginId, key);
			kvEntries = kvEntries.filter((k) => k.key !== key);
		} catch (e) {
			alert('Failed to delete KV key: ' + e);
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
					Install a sandboxed WASM MCP-App binary, attach a local stdio process, or connect a remote Streamable HTTP MCP server.
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
							<th class="hidden p-3 md:table-cell">Permissions</th>
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
													title={srv.transport === 'http' ? 'Custom HTTP Headers / Auth configured' : 'Custom Environment Variables configured'}
												>
													<Key class="h-2.5 w-2.5" />
													<span>{srv.transport === 'http' ? 'Auth' : 'Env'}</span>
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
										<span class="rounded-full px-2 py-0.5 text-[10px] font-mono uppercase {srv.transport === 'wasm_stdio' ? 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20' : srv.transport === 'custom_tools' || srv.transport === 'curl' ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20' : srv.transport === 'stdio' ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20' : 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400 border border-cyan-500/20'}">
											{srv.transport === 'wasm_stdio' ? 'WASM' : srv.transport === 'custom_tools' || srv.transport === 'curl' ? 'CUSTOM cURL' : srv.transport}
										</span>
										{#if srv.transport === 'wasm_stdio'}
											<span class="text-[10px] text-zinc-400 font-mono">
												{formatBytes(srv.storage_bytes || 0)}
											</span>
										{/if}
									</div>
								</td>
								<td class="hidden md:table-cell p-3">
									<div class="flex flex-wrap items-center gap-1">
										{#if perms.ui.enabled}
											<span
												class="rounded-md border border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300 px-2 py-0.5 text-[10px] font-medium"
												title="{perms.ui.resources.length && !perms.ui.resources.includes('*') ? `Renders MCP-App views: ${perms.ui.resources.join(', ')}` : 'Renders MCP-App UI views (ui:// resources)'}"
											>
												UI Hook
											</span>
										{/if}
										{#if perms.storage.get || perms.storage.set || perms.storage.delete}
											<span
												class="rounded-md border border-sky-500/30 bg-sky-500/10 text-sky-600 dark:text-sky-300 px-2 py-0.5 text-[10px] font-medium"
												title="{`Isolated storage: ${[perms.storage.get ? 'read' : null, perms.storage.set ? 'write' : null, perms.storage.delete ? 'delete' : null].filter(Boolean).join(' / ')}${perms.storage.keys && perms.storage.keys.length ? ` (keys: ${perms.storage.keys.join(', ')}*)` : ' (all keys)'}`}"
											>
												Storage
											</span>
										{/if}
										{#if perms.process}
											<span
												class="rounded-md border border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-300 px-2 py-0.5 text-[10px] font-medium"
												title="Spawns a local OS child process"
											>
												Process
											</span>
										{/if}
										{#if perms.network}
											<span
												class="rounded-md border border-purple-500/30 bg-purple-500/10 text-purple-600 dark:text-purple-300 px-2 py-0.5 text-[10px] font-medium"
												title="Makes outbound HTTP/SSE network requests"
											>
												Network
											</span>
										{/if}
										{#if perms.agent.trigger}
											<span
												class="rounded-md border border-zinc-400/30 bg-zinc-500/10 text-zinc-600 dark:text-zinc-300 px-2 py-0.5 text-[10px] font-medium"
												title="Can start an agent turn with text it composes"
											>
												Agent Trigger
											</span>
										{/if}
										{#if perms.agent.tools}
											<span
												class="rounded-md border border-zinc-400/30 bg-zinc-500/10 text-zinc-600 dark:text-zinc-300 px-2 py-0.5 text-[10px] font-medium"
												title="Exposes its tools to the agent loop"
											>
												Agent Tools
											</span>
										{/if}
										{#if perms.identity['agent.name']}
											<span
												class="rounded-md border border-teal-500/30 bg-teal-500/10 text-teal-600 dark:text-teal-300 px-2 py-0.5 text-[10px] font-medium"
												title="Sends the bound agent&#39;s display name into the plugin UI"
											>
												Agent Name
											</span>
										{/if}
										{#if perms.identity['agent.profile_picture']}
											<span
												class="rounded-md border border-teal-500/30 bg-teal-500/10 text-teal-600 dark:text-teal-300 px-2 py-0.5 text-[10px] font-medium"
												title="Sends the bound agent&#39;s profile picture into the plugin UI"
											>
												Agent Picture
											</span>
										{/if}
										{#if perms.identity['user.name']}
											<span
												class="rounded-md border border-teal-500/30 bg-teal-500/10 text-teal-600 dark:text-teal-300 px-2 py-0.5 text-[10px] font-medium"
												title="Sends your display name into the plugin UI"
											>
												Your Name
											</span>
										{/if}
										{#if perms.identity['user.profile_picture']}
											<span
												class="rounded-md border border-teal-500/30 bg-teal-500/10 text-teal-600 dark:text-teal-300 px-2 py-0.5 text-[10px] font-medium"
												title="Sends your profile picture into the plugin UI"
											>
												Your Picture
											</span>
										{/if}
									</div>
								</td>
								<td class="hidden md:table-cell p-3 font-mono text-[11px] text-zinc-500 dark:text-zinc-400 max-w-xs truncate" title={srv.url_or_command}>
									{srv.url_or_command}
								</td>
								<td class="p-2.5 pr-3 md:p-3 md:pr-4 text-right">
									<div class="flex items-center justify-end gap-1.5">
										{#if srv.transport === 'wasm_stdio'}
											<button
												type="button"
												class="flex items-center gap-1 rounded-lg bg-indigo-500/10 border border-indigo-500/25 px-2 py-1 text-[11px] font-medium text-indigo-600 dark:text-indigo-400 hover:bg-indigo-500/20 transition-colors"
												onclick={() => openEditModal(srv)}
												title="Update or re-upload WASM binary"
											>
												<RefreshCw class="h-3 w-3 text-indigo-500" />
												<span>Update</span>
											</button>
											<button
												type="button"
												class="flex items-center gap-1 rounded-lg bg-zinc-100 dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 px-2 py-1 text-[11px] font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors"
												onclick={() => inspectWasmStorage(srv)}
												title="Inspect isolated WASM KV & storage"
											>
												<Database class="h-3 w-3 text-sky-500" />
												<span>Data</span>
											</button>
										{/if}


										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-800 dark:hover:text-white transition-colors"
											onclick={() => openEditModal(srv)}
											title="Edit Plugin & Permissions"
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

		<!-- WASM Storage & KV Inspector Drawer -->
		{#if selectedStoragePlugin}
			<section class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/60 p-5 shadow-xs space-y-4">
				<div class="flex items-center justify-between border-b border-zinc-200/60 dark:border-zinc-800 pb-3">
					<div class="flex items-center gap-2">
						<Database class="h-4 w-4 text-sky-500" />
						<h3 class="text-xs font-semibold text-zinc-900 dark:text-white">
							Isolated Storage &amp; KV Inspector — {selectedStoragePlugin.name}
						</h3>
						{#if storageInfo}
							<span class="rounded bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 font-mono text-[10px] text-zinc-500">
								sha256: {storageInfo.wasm_hash.slice(0, 12)}... ({formatBytes(storageInfo.total_storage_bytes)})
							</span>
						{/if}
					</div>
					<button
						type="button"
						class="text-xs text-zinc-400 hover:text-zinc-700 dark:hover:text-white"
						onclick={() => (selectedStoragePlugin = null)}
					>
						Close
					</button>
				</div>

				{#if loadingStorage}
					<div class="py-6 text-center text-xs text-zinc-400">Loading plugin storage...</div>
				{:else}
					<div class="space-y-2">
						<div class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
							Key-Value Entries ({kvEntries.length})
						</div>
						{#if kvEntries.length === 0}
							<p class="text-xs text-zinc-500 italic">No KV entries stored by this plugin.</p>
						{:else}
							<div class="divide-y divide-zinc-200/60 dark:divide-zinc-800 rounded-xl border border-zinc-200/80 dark:border-zinc-800">
								{#each kvEntries as entry (entry.key)}
									<div class="flex items-start justify-between gap-4 p-3 text-xs">
										<div class="min-w-0 flex-1">
											<div class="font-mono font-semibold text-indigo-600 dark:text-indigo-400">{entry.key}</div>
											<pre class="mt-1 overflow-x-auto whitespace-pre-wrap break-all font-mono text-[11px] text-zinc-600 dark:text-zinc-300">{entry.value}</pre>
										</div>
										<button
											type="button"
											onclick={() => selectedStoragePlugin && deleteKvEntry(selectedStoragePlugin.id, entry.key)}
											class="rounded border border-rose-500/20 bg-rose-500/10 px-2 py-1 text-[10px] text-rose-500 hover:bg-rose-500/20"
										>
											Delete Key
										</button>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
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
									formTransport = 'wasm';
									formPermissions = parsePermissionsFromManifest(wasmInspectPending?.manifest, 'wasm');
								}}
								class="flex flex-col items-center gap-1 rounded-xl border p-2.5 text-xs font-medium transition-all cursor-pointer {formTransport === 'wasm' ? 'border-indigo-500 bg-indigo-500/10 text-indigo-600 dark:text-indigo-300' : 'border-zinc-200 dark:border-zinc-800 text-zinc-600 dark:text-zinc-400'}"
							>
								<Layers class="h-4 w-4" />
								<span>WASM Binary</span>
							</button>
							<button
								type="button"
								onclick={() => {
									formTransport = 'stdio';
									formPermissions = parsePermissionsFromManifest(null, 'stdio');
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

				{#if formTransport === 'wasm'}
					<!-- WASM Upload or URL -->
					<div class="space-y-3 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-800/30 p-4">
						{#if editingServer}
							<div class="rounded-xl border border-zinc-200 dark:border-zinc-700/60 bg-white dark:bg-zinc-900 p-3 text-xs space-y-1.5">
								<div class="flex items-center justify-between">
									<div class="flex items-center gap-2">
										<Layers class="h-4 w-4 text-indigo-500" />
										<span class="font-semibold text-zinc-900 dark:text-zinc-100">Installed Binary</span>
										<span class="rounded bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20 px-1.5 py-0.2 font-mono text-[10px]">v{editingServer.version || '1.0.0'}</span>
									</div>
									<span class="text-[10px] text-zinc-400 font-mono">Storage: {formatBytes(editingServer.storage_bytes || 0)}</span>
								</div>
								{#if editingServer.wasm_hash}
									<div class="text-[10px] font-mono text-zinc-400 dark:text-zinc-500 truncate" title={editingServer.wasm_hash}>
										Hash: {editingServer.wasm_hash}
									</div>
								{/if}
							</div>
						{/if}

						<label class="flex flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed border-zinc-300 dark:border-zinc-700 p-4 text-center cursor-pointer hover:border-indigo-500 transition-colors bg-white/50 dark:bg-zinc-900/50">
							<Upload class="h-5 w-5 text-indigo-500" />
							<span class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
								{isInspectingWasm ? 'Inspecting WASM binary...' : editingServer ? 'Select New .wasm Binary to Update / Upgrade' : 'Select .wasm Plugin File'}
							</span>
							<span class="text-[11px] text-zinc-400">Exports get_manifest &amp; mcp_handle_request</span>
							<input type="file" accept=".wasm" onchange={handleWasmFileSelected} class="hidden" />
						</label>

						{#if wasmInspectPending}
							<div class="rounded-xl border border-indigo-500/30 bg-indigo-500/10 p-3 text-xs space-y-1">
								<div class="flex items-center justify-between">
									<span class="font-semibold text-indigo-300">Ready to update: {wasmInspectPending.name} v{wasmInspectPending.version}</span>
									<span class="font-mono text-[10px] text-indigo-400">id: {wasmInspectPending.id}</span>
								</div>
								{#if wasmInspectPending.description}
									<p class="text-[11px] text-zinc-300">{wasmInspectPending.description}</p>
								{/if}
								{#if wasmInspectPending.manifest?.mcp?.resources?.length}
									<div class="pt-1 text-[11px] text-emerald-300">
										MCP-App UI Hooks: {wasmInspectPending.manifest.mcp.resources.map((r: any) => r.uri).join(', ')}
									</div>
								{/if}
							</div>
						{/if}

						<div class="relative flex py-1 items-center">
							<div class="grow border-t border-zinc-200 dark:border-zinc-800"></div>
							<span class="shrink mx-2 text-[10px] uppercase text-zinc-400">{editingServer ? 'Or update from URL' : 'Or install via URL'}</span>
							<div class="grow border-t border-zinc-200 dark:border-zinc-800"></div>
						</div>

						<input
							type="text"
							bind:value={wasmUrl}
							placeholder="https://example.com/plugin.wasm"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white"
						/>
					</div>
				{:else if formTransport === 'custom_tools'}
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

									{#if currentTool.parsed.cookie_expression || currentTool.parsed.captures.cookies}
										<div class="flex flex-wrap items-center gap-2 pt-1 border-t border-zinc-100 dark:border-zinc-800 text-[11px]">
											{#if currentTool.parsed.captures.cookies}
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
									{#if currentTool.parsed.variables.length > 0}
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

										{#if currentTool.parsed.variables.length > 0}
											<div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
												{#each currentTool.parsed.variables as v}
													<div>
														<label for="test-{v.name}" class="block text-[10px] text-zinc-500 mb-0.5">Test value for {v.name}</label>
														<input
															id="test-{v.name}"
															type="text"
															bind:value={currentTool.testArgs[v.name]}
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

					{#if editingServer?.transport !== 'wasm_stdio'}
						<div>
							<label for="mcp-cmd" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
								{formTransport === 'stdio' ? 'Binary / Command + Arguments' : 'Streamable HTTP / SSE Endpoint URL'}
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

						<div>
							<label for="mcp-headers" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
								{formTransport === 'http' ? 'Headers & Authentication (Optional)' : 'Environment Variables (Optional)'}
							</label>
							<textarea
								id="mcp-headers"
								rows="3"
								bind:value={formHeadersOrEnv}
								placeholder={formTransport === 'http'
									? '{"Authorization": "Bearer YOUR_API_KEY"}\nor\nAuthorization: Bearer YOUR_API_KEY'
									: '{"GITHUB_TOKEN": "ghp_xxx"}\nor\nKEY=VALUE'}
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
							></textarea>
						</div>
					{/if}
				{/if}

<!-- Required Access (read-only: what the plugin declares it needs) -->
<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50/70 dark:bg-zinc-950/70 p-3.5 space-y-2.5">
<div class="flex items-center justify-between">
<div class="flex items-center gap-1.5 text-xs font-semibold text-zinc-800 dark:text-zinc-200">
<Shield class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
<span>Required Access</span>
</div>
<span class="text-[10px] text-zinc-400">Declared by the plugin — not configurable</span>
</div>

{#if formRequiredAccess.length === 0}
<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
This plugin requests no special access. It only exposes the tools listed above.
</p>
{:else}
<ul class="space-y-1">
{#each formRequiredAccess as item (item.scope)}
<li class="flex items-start gap-2 rounded-lg bg-white dark:bg-zinc-900/60 px-2.5 py-1.5">
<span class="mt-0.5 shrink-0 rounded-md bg-zinc-100 dark:bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-600 dark:text-zinc-300">{item.scope}</span>
<span class="min-w-0 text-[10px] leading-snug text-zinc-500 dark:text-zinc-400">{item.description}</span>
</li>
{/each}
</ul>
{/if}
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
							(formTransport === 'wasm'
								? !editingServer && !wasmInspectPending && !wasmUrl.trim()
								: formTransport === 'custom_tools'
									? !formName.trim() || customTools.length === 0 || !customTools[0]?.parsed?.url
									: !formName.trim() || !formUrlOrCommand.trim())}
					>
						{isSaving
							? 'Saving...'
							: formTransport === 'wasm' && wasmInspectPending
								? 'Update WASM Binary'
								: editingServer
									? 'Update Plugin'
									: 'Save Plugin'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
