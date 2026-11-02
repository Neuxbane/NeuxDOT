<script lang="ts">
	import { api, type ExtensionItem, type ExtensionKvEntry, type ExtensionStorageInfo } from '$lib/api';
	import { authState } from '$lib/stores/auth.svelte';
	import {
		Trash2,
		X,
		Package,
		Upload,
		Database,
		HardDrive,
		RefreshCw,
		FileText,
		ExternalLink,
		ShieldCheck
	} from 'lucide-svelte';

	// Extensions State
	let extensionsList = $state<ExtensionItem[]>([]);
	let isLoadingExtensions = $state(false);
	let isUploadingWasm = $state(false);
	let installUrlInput = $state('');
	let isInstallingUrl = $state(false);
	let fileInputRef: HTMLInputElement | null = null;

	// Extension Update Comparison Modal State
	let pendingUpdateInspection = $state<any>(null);
	let pendingUpdateFile = $state<File | null>(null);
	let isComparingUpdate = $state(false);

	// Extension Inspector Drawer State
	let selectedExt = $state<ExtensionItem | null>(null);
	let inspectingKv = $state<ExtensionKvEntry[]>([]);
	let inspectingStorage = $state<ExtensionStorageInfo | null>(null);
	let isLoadingInspector = $state(false);

	$effect(() => {
		if (authState.isAuthenticated) {
			loadExtensions();
		}
	});

	async function loadExtensions() {
		isLoadingExtensions = true;
		try {
			extensionsList = await api.extensions.list();
			if (selectedExt) {
				const refreshed = extensionsList.find((e) => e.id === selectedExt?.id);
				if (refreshed) {
					selectedExt = refreshed;
					loadInspectorData(refreshed);
				} else {
					selectedExt = null;
				}
			}
		} catch (e) {
			console.error('Failed to load extensions:', e);
		} finally {
			isLoadingExtensions = false;
		}
	}

	async function handleUploadWasmFile(event: Event) {
		const target = event.target as HTMLInputElement;
		const file = target.files?.[0];
		if (!file) return;

		isUploadingWasm = true;
		try {
			// First, inspect the WASM binary to check if it's an update to an existing extension
			const inspection = await api.extensions.inspect(file);
			if (inspection.existing) {
				pendingUpdateInspection = inspection;
				pendingUpdateFile = file;
				isComparingUpdate = true;
				return;
			}

			// If not existing, install directly
			await api.extensions.upload(file);
			target.value = '';
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to inspect/upload extension: ' + (e?.message || e));
		} finally {
			isUploadingWasm = false;
		}
	}

	async function confirmExtensionUpdate() {
		if (!pendingUpdateFile) return;
		isUploadingWasm = true;
		try {
			await api.extensions.upload(pendingUpdateFile);
			if (fileInputRef) fileInputRef.value = '';
			isComparingUpdate = false;
			pendingUpdateInspection = null;
			pendingUpdateFile = null;
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to apply extension update: ' + (e?.message || e));
		} finally {
			isUploadingWasm = false;
		}
	}

	function cancelExtensionUpdate() {
		isComparingUpdate = false;
		pendingUpdateInspection = null;
		pendingUpdateFile = null;
		if (fileInputRef) fileInputRef.value = '';
	}

	async function handleInstallUrl() {
		const url = installUrlInput.trim();
		if (!url) return;

		isInstallingUrl = true;
		try {
			await api.extensions.installUrl(url);
			installUrlInput = '';
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to install from URL: ' + (e?.message || e));
		} finally {
			isInstallingUrl = false;
		}
	}

	async function toggleExtensionActive(ext: ExtensionItem) {
		try {
			await api.extensions.patch(ext.id, !ext.is_active);
			extensionsList = extensionsList.map((e) =>
				e.id === ext.id ? { ...e, is_active: !ext.is_active } : e
			);
		} catch (e: any) {
			alert('Failed to toggle extension: ' + (e?.message || e));
		}
	}

	async function deleteExtension(id: string) {
		if (!confirm('Are you sure you want to uninstall this extension? All isolated KV and file storage will be permanently wiped.'))
			return;

		try {
			await api.extensions.delete(id);
			if (selectedExt?.id === id) {
				selectedExt = null;
			}
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to delete extension: ' + (e?.message || e));
		}
	}

	async function selectExtensionForInspect(ext: ExtensionItem) {
		selectedExt = ext;
		await loadInspectorData(ext);
	}

	async function loadInspectorData(ext: ExtensionItem) {
		isLoadingInspector = true;
		try {
			const [kv, storage] = await Promise.all([
				api.extensions.getKv(ext.id),
				api.extensions.getStorage(ext.id)
			]);
			inspectingKv = kv;
			inspectingStorage = storage;
		} catch (e) {
			console.error('Failed to load inspector data:', e);
		} finally {
			isLoadingInspector = false;
		}
	}

	async function deleteKvKey(key: string) {
		if (!selectedExt) return;
		if (!confirm(`Delete KV key "${key}"?`)) return;

		try {
			await api.extensions.deleteKvKey(selectedExt.id, key);
			inspectingKv = inspectingKv.filter((k) => k.key !== key);
			await loadInspectorData(selectedExt);
		} catch (e: any) {
			alert('Failed to delete key: ' + (e?.message || e));
		}
	}

	function formatBytes(bytes: number): string {
		if (bytes === 0) return '0 B';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
	}

</script>

<!-- Extensions & Storage (moved verbatim from the old monolithic settings page) -->
<section id="extensions" class="scroll-mt-8 space-y-6">
	<div class="flex items-center justify-between">
		<div>
			<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">
				WASM Extensions & Isolated Storage
			</h2>
			<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
				Install compiled WASM binaries. Each extension has its own isolated KV store and disk filesystem partitioned by WASM hash.
			</p>
		</div>

		<button
			type="button"
			class="flex items-center gap-1.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-50 dark:hover:bg-zinc-800 transition-colors shadow-2xs"
			onclick={loadExtensions}
			disabled={isLoadingExtensions}
			title="Refresh extensions list"
		>
			<RefreshCw class="h-3.5 w-3.5 {isLoadingExtensions ? 'animate-spin' : ''}" />
			<span>Refresh</span>
		</button>
	</div>

	<!-- Installation Area (File Upload & URL) -->
	<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 space-y-3 shadow-xs">
		<span class="block text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
			Install New Extension
		</span>

		<div class="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1">
			<!-- Upload File -->
			<div class="flex flex-col justify-between rounded-xl border border-dashed border-zinc-300 dark:border-zinc-750 p-3 bg-zinc-50/50 dark:bg-zinc-950/40">
				<div>
					<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
						<Upload class="h-4 w-4 text-emerald-500" />
						<span>Select .wasm binary</span>
					</div>
					<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-1">
						Upload compiled WebAssembly file directly from disk (e.g. from <code>./examples/chat-extension/target/wasm32-unknown-unknown/release/chat_extension.wasm</code>).
					</p>
				</div>

				<div class="pt-3">
					<input
						type="file"
						accept=".wasm"
						class="hidden"
						bind:this={fileInputRef}
						onchange={handleUploadWasmFile}
					/>
					<button
						type="button"
						class="w-full flex items-center justify-center gap-1.5 rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-40"
						onclick={() => fileInputRef?.click()}
						disabled={isUploadingWasm}
					>
						<Upload class="h-3.5 w-3.5" />
						<span>{isUploadingWasm ? 'Uploading...' : 'Choose .wasm File'}</span>
					</button>
				</div>
			</div>

			<!-- Install via URL -->
			<div class="flex flex-col justify-between rounded-xl border border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-50/50 dark:bg-zinc-950/40">
				<div>
					<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
						<ExternalLink class="h-4 w-4 text-cyan-500" />
						<span>Download from URL</span>
					</div>
					<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-1">
						Download a remote .wasm binary directly over HTTP/HTTPS.
					</p>
				</div>

				<div class="pt-3 flex gap-2">
					<input
						type="url"
						bind:value={installUrlInput}
						placeholder="https://.../module.wasm"
						class="flex-1 rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none"
					/>
					<button
						type="button"
						class="rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-40"
						onclick={handleInstallUrl}
						disabled={!installUrlInput.trim() || isInstallingUrl}
					>
						{isInstallingUrl ? '...' : 'Install'}
					</button>
				</div>
			</div>
		</div>
	</div>

	<!-- Installed Extensions List -->
	<div class="space-y-3">
		<span class="block text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
			Installed Extensions ({extensionsList.length})
		</span>

		{#if extensionsList.length === 0}
			<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
				No extensions installed yet. Upload your compiled <code>chat_extension.wasm</code> above to test.
			</div>
		{:else}
			<div class="grid grid-cols-1 gap-3">
				{#each extensionsList as ext (ext.id)}
					{@const isSelected = selectedExt?.id === ext.id}
					<div
						class="rounded-2xl border p-4 transition-all {isSelected
							? 'border-emerald-500/60 dark:border-emerald-500/60 bg-emerald-50/20 dark:bg-emerald-950/10 shadow-xs'
							: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50 dark:hover:bg-zinc-800/40'}"
					>
						<div class="flex items-start justify-between">
							<div class="flex items-start gap-3">
								<div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20 font-bold text-sm">
									<Package class="h-5 w-5" />
								</div>
								<div>
									<div class="flex items-center gap-2">
										<span class="text-sm font-semibold text-zinc-900 dark:text-white">
											{ext.name}
										</span>
										<span class="rounded-full bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-mono text-zinc-500">
											v{ext.version}
										</span>
										{#if ext.is_active}
											<span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
												Active
											</span>
										{:else}
											<span class="rounded-full bg-zinc-200/60 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-medium text-zinc-500">
												Disabled
											</span>
										{/if}
									</div>

									{#if ext.description}
										<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-1">
											{ext.description}
										</p>
									{/if}

									<!-- Metadata Badges -->
									<div class="mt-2 flex flex-wrap items-center gap-2">
										<span class="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 border border-zinc-200 dark:border-zinc-700">
											<HardDrive class="h-3 w-3" />
											{formatBytes(ext.storage_bytes)}
										</span>
										<span
											class="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700"
											title={ext.wasm_hash}
										>
											sha256: {ext.wasm_hash.substring(0, 10)}...
										</span>
										{#if ext.manifest.ui_hooks}
											{#each ext.manifest.ui_hooks as hook}
												<span class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono bg-indigo-50 dark:bg-indigo-950/60 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-800">
													Slot: #{hook.target_id}
												</span>
											{/each}
										{/if}
									</div>
								</div>
							</div>

							<div class="flex items-center gap-2">
								<!-- Active Toggle -->
								<button
									type="button"
									aria-label="Toggle extension active state"
									class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {ext.is_active
										? 'bg-emerald-500'
										: 'bg-zinc-300 dark:bg-zinc-700'}"
									onclick={() => toggleExtensionActive(ext)}
								>
									<span
										class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {ext.is_active
											? 'translate-x-4'
											: 'translate-x-0'}"
									></span>
								</button>

								<!-- Inspect Storage & KV Button -->
								<button
									type="button"
									class="rounded-xl border border-zinc-200 dark:border-zinc-750 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors shadow-2xs flex items-center gap-1.5"
									onclick={() => selectExtensionForInspect(ext)}
								>
									<Database class="h-3.5 w-3.5 text-cyan-500" />
									<span>Inspect Storage</span>
								</button>

								<!-- Delete Button -->
								<button
									type="button"
									class="rounded-lg p-1.5 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
									onclick={() => deleteExtension(ext.id)}
									title="Uninstall extension"
									aria-label="Uninstall extension"
								>
									<Trash2 class="h-4 w-4" />
								</button>
							</div>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Inspector Drawer: Storage Breakdown & KV Table -->
	{#if selectedExt}
		<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-5 space-y-5 shadow-xs">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<div class="flex items-center gap-2">
					<Database class="h-4 w-4 text-cyan-500" />
					<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
						Storage Inspector: {selectedExt.name}
					</h3>
				</div>

				<div class="flex items-center gap-3">
					<button
						type="button"
						class="text-xs text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 flex items-center gap-1"
						onclick={() => loadInspectorData(selectedExt!)}
					>
						<RefreshCw class="h-3 w-3 {isLoadingInspector ? 'animate-spin' : ''}" />
						<span>Refresh Data</span>
					</button>
					<button
						type="button"
						class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white"
						onclick={() => (selectedExt = null)}
						aria-label="Close inspector"
					>
						<X class="h-4 w-4" />
					</button>
				</div>
			</div>

			<!-- Storage Resource Stats -->
			<div class="grid grid-cols-3 gap-3 text-xs">
				<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
					<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">Total Storage</span>
					<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
						{formatBytes(inspectingStorage?.total_storage_bytes || selectedExt.storage_bytes)}
					</span>
				</div>
				<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
					<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">KV Entries</span>
					<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
						{inspectingKv.length} keys
					</span>
				</div>
				<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
					<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">Disk Files</span>
					<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
						{inspectingStorage?.files.length || 0} files
					</span>
				</div>
			</div>

			<!-- Isolated Filesystem Files -->
			{#if inspectingStorage && inspectingStorage.files.length > 0}
				<div class="space-y-2">
					<span class="block text-xs font-semibold text-zinc-800 dark:text-zinc-200">
						Isolated Filesystem Files (<code>data/extensions/{selectedExt.wasm_hash.substring(0, 12)}.../</code>)
					</span>
					<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/50 divide-y divide-zinc-100 dark:divide-zinc-800/80">
						{#each inspectingStorage.files as file}
							<div class="flex items-center justify-between p-2.5 text-xs font-mono">
								<div class="flex items-center gap-2 truncate">
									<FileText class="h-3.5 w-3.5 text-zinc-400 shrink-0" />
									<span class="text-zinc-800 dark:text-zinc-200 truncate">{file.name}</span>
								</div>
								<span class="text-zinc-400 text-[11px] shrink-0 ml-2">
									{formatBytes(file.size_bytes)}
								</span>
							</div>
						{/each}
					</div>
				</div>
			{/if}

			<!-- KV Table -->
			<div class="space-y-2">
				<span class="block text-xs font-semibold text-zinc-800 dark:text-zinc-200">
					Host-Managed KV Store Table
				</span>

				{#if inspectingKv.length === 0}
					<div class="rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 p-6 text-center text-xs text-zinc-400">
						No KV keys stored by this extension yet.
					</div>
				{:else}
					<div class="overflow-hidden rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-950 shadow-2xs">
						<table class="w-full text-left text-xs font-mono">
							<thead class="border-b border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-500 font-sans text-[11px]">
								<tr>
									<th class="p-2.5 pl-3">Key</th>
									<th class="p-2.5">Value (JSON/String)</th>
									<th class="p-2.5">Updated</th>
									<th class="p-2.5 text-right pr-3">Action</th>
								</tr>
							</thead>
							<tbody class="divide-y divide-zinc-100 dark:divide-zinc-800/80">
								{#each inspectingKv as item}
									<tr class="hover:bg-zinc-50 dark:hover:bg-zinc-900/40 transition-colors">
										<td class="p-2.5 pl-3 font-semibold text-indigo-600 dark:text-indigo-400">
											{item.key}
										</td>
										<td class="p-2.5 max-w-xs truncate text-zinc-700 dark:text-zinc-300">
											{item.value}
										</td>
										<td class="p-2.5 text-[11px] text-zinc-400 font-sans">
											{item.updated_at ? new Date(item.updated_at).toLocaleTimeString() : 'N/A'}
										</td>
										<td class="p-2.5 text-right pr-3 font-sans">
											<button
												type="button"
												class="text-zinc-400 hover:text-red-500 transition-colors"
												onclick={() => deleteKvKey(item.key)}
												title="Delete KV key"
											>
												<Trash2 class="h-3.5 w-3.5 inline" />
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</div>
		</div>
	{/if}
</section>

{#if isComparingUpdate && pendingUpdateInspection}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4">
		<div class="w-full max-w-lg rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl space-y-4">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<div class="flex items-center gap-2">
					<div class="flex h-8 w-8 items-center justify-center rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20">
						<Package class="h-4 w-4" />
					</div>
					<div>
						<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Extension Update Comparison</h3>
						<p class="text-[11px] text-zinc-500 dark:text-zinc-400">Review changes before replacing the existing extension binary</p>
					</div>
				</div>
				<button
					type="button"
					class="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200"
					onclick={cancelExtensionUpdate}
				>
					<X class="h-4 w-4" />
				</button>
			</div>

			<!-- Comparison Cards -->
			<div class="grid grid-cols-2 gap-3 text-xs">
				<!-- Old Extension Info -->
				<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3 space-y-2">
					<div class="flex items-center justify-between">
						<span class="font-semibold text-zinc-700 dark:text-zinc-300">Installed Version</span>
						<span class="rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-200 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300">v{pendingUpdateInspection.existing.version}</span>
					</div>
					<div class="space-y-1 text-[11px] text-zinc-500 dark:text-zinc-400">
						<div><span class="text-zinc-400">Name:</span> <strong class="text-zinc-800 dark:text-zinc-200">{pendingUpdateInspection.existing.name}</strong></div>
						<div><span class="text-zinc-400">Hash:</span> <code class="font-mono text-[10px]">{pendingUpdateInspection.existing.wasm_hash.slice(0, 12)}...</code></div>
						<div><span class="text-zinc-400">Storage:</span> <span class="font-mono text-indigo-600 dark:text-indigo-400">{formatBytes(pendingUpdateInspection.existing.storage_bytes)}</span></div>
					</div>
				</div>

				<!-- New Extension Info -->
				<div class="rounded-xl border border-indigo-500/30 bg-indigo-50/50 dark:bg-indigo-950/20 p-3 space-y-2">
					<div class="flex items-center justify-between">
						<span class="font-semibold text-indigo-700 dark:text-indigo-300">Incoming Version</span>
						<span class="rounded px-1.5 py-0.5 text-[10px] font-mono bg-indigo-500/20 text-indigo-700 dark:text-indigo-300">v{pendingUpdateInspection.version}</span>
					</div>
					<div class="space-y-1 text-[11px] text-zinc-500 dark:text-zinc-400">
						<div><span class="text-zinc-400">Name:</span> <strong class="text-zinc-800 dark:text-zinc-200">{pendingUpdateInspection.name}</strong></div>
						<div><span class="text-zinc-400">Hash:</span> <code class="font-mono text-[10px]">{pendingUpdateInspection.wasm_hash.slice(0, 12)}...</code></div>
						<div><span class="text-zinc-400">Status:</span> <span class="text-emerald-600 dark:text-emerald-400 font-medium">Ready to apply</span></div>
					</div>
				</div>
			</div>

			<!-- Zero-Trust Migration Notice -->
			<div class="rounded-xl border border-emerald-500/20 bg-emerald-500/5 p-3 text-[11px] text-emerald-800 dark:text-emerald-300 flex items-start gap-2">
				<ShieldCheck class="h-4 w-4 shrink-0 mt-0.5" />
				<div>
					<strong>Automatic Data Migration:</strong> All existing KV tables and persistent storage files will be seamlessly moved to the new hash directory. The old hash directory will be purged to keep storage clean.
				</div>
			</div>

			<!-- Action buttons -->
			<div class="flex items-center justify-end gap-2 pt-2 border-t border-zinc-100 dark:border-zinc-800">
				<button
					type="button"
					class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
					onclick={cancelExtensionUpdate}
				>
					Cancel
				</button>
				<button
					type="button"
					class="rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white px-4 py-2 text-xs font-medium transition-all shadow-xs disabled:opacity-40"
					disabled={isUploadingWasm}
					onclick={confirmExtensionUpdate}
				>
					{isUploadingWasm ? 'Updating & Migrating...' : 'Confirm Update & Migrate Data'}
				</button>
			</div>
		</div>
	</div>
{/if}
