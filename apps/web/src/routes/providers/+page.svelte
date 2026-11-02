<script lang="ts">
	import { api, type Provider, type ModelConfigPayload } from '$lib/api';
	import { navState } from '$lib/stores/nav.svelte';
	import {
		Cpu,
		Plus,
		Trash2,
		Edit3,
		X,
		ArrowLeft,
		RefreshCw,
		Key,
		Layers,
		AlertCircle,
		Check,
		Sliders,
		ChevronDown,
		Sparkles
	} from 'lucide-svelte';

	interface FormModelItem {
		id: string;
		model_id: string;
		name?: string;
		context_window?: number;
		max_tokens?: number;
		temperature?: number;
		top_p?: number;
		thinking_enabled: boolean;
		thinking_budget?: number;
		custom_json?: string;
		isExpanded?: boolean;
	}

	let providers = $state<Provider[]>([]);
	let isLoading = $state(true);

	let isModalOpen = $state(false);
	let editingProvider = $state<Provider | null>(null);

	let formName = $state('');
	let formBaseUrl = $state('https://api.openai.com/v1');
	let formApiKey = $state('');
	let formModels = $state<FormModelItem[]>([]);
	let newModelInput = $state('');

	let isFetchingModels = $state(false);
	let fetchError = $state<string | null>(null);
	let fetchSuccessMessage = $state<string | null>(null);

	let isSaving = $state(false);

	$effect(() => {
		loadProviders();
	});

	async function loadProviders() {
		isLoading = true;
		try {
			providers = await api.providers.list();
		} catch (e) {
			console.error('Failed to load providers:', e);
		} finally {
			isLoading = false;
		}
	}

	function parseModelItem(model_id: string, name?: string, config_json?: string): FormModelItem {
		let context_window: number | undefined;
		let max_tokens: number | undefined;
		let temperature: number | undefined = 0.7;
		let top_p: number | undefined;
		let thinking_enabled = false;
		let thinking_budget: number | undefined = 2048;
		let custom_json = '';

		if (config_json) {
			try {
				const p = JSON.parse(config_json);
				if (p.context_window) context_window = Number(p.context_window);
				if (p.max_tokens) max_tokens = Number(p.max_tokens);
				if (p.temperature !== undefined) temperature = Number(p.temperature);
				if (p.top_p !== undefined) top_p = Number(p.top_p);
				if (p.thinking) {
					thinking_enabled = true;
					if (p.thinking.budget_tokens) thinking_budget = Number(p.thinking.budget_tokens);
				}

				const rem = { ...p };
				delete rem.context_window;
				delete rem.max_tokens;
				delete rem.temperature;
				delete rem.top_p;
				delete rem.thinking;

				if (Object.keys(rem).length > 0) {
					custom_json = JSON.stringify(rem, null, 2);
				}
			} catch {}
		}

		return {
			id: Math.random().toString(36).substring(2, 9),
			model_id,
			name,
			context_window,
			max_tokens,
			temperature,
			top_p,
			thinking_enabled,
			thinking_budget,
			custom_json,
			isExpanded: false
		};
	}

	function serializeModelItem(item: FormModelItem): ModelConfigPayload {
		const cfg: Record<string, any> = {};
		if (item.context_window && !isNaN(item.context_window)) cfg.context_window = item.context_window;
		if (item.max_tokens && !isNaN(item.max_tokens)) cfg.max_tokens = item.max_tokens;
		if (item.temperature !== undefined && !isNaN(item.temperature)) cfg.temperature = item.temperature;
		if (item.top_p !== undefined && !isNaN(item.top_p)) cfg.top_p = item.top_p;
		if (item.thinking_enabled) {
			cfg.thinking = {
				type: 'enabled',
				budget_tokens: item.thinking_budget || 2048
			};
		}
		if (item.custom_json?.trim()) {
			try {
				const parsed = JSON.parse(item.custom_json.trim());
				Object.assign(cfg, parsed);
			} catch {}
		}

		return {
			model_id: item.model_id,
			name: item.name?.trim() || undefined,
			config_json: Object.keys(cfg).length > 0 ? JSON.stringify(cfg) : undefined
		};
	}

	function openCreateModal() {
		editingProvider = null;
		formName = '';
		formBaseUrl = 'https://api.openai.com/v1';
		formApiKey = '';
		formModels = [parseModelItem('gpt-4o', undefined, '{"context_window":128000,"max_tokens":4096}')];
		newModelInput = '';
		fetchError = null;
		fetchSuccessMessage = null;
		isModalOpen = true;
	}

	function openEditModal(p: Provider) {
		editingProvider = p;
		formName = p.name;
		formBaseUrl = p.base_url;
		formApiKey = p.api_key || '';
		formModels = (p.models || []).map((m) => parseModelItem(m.model_id, m.name, m.config_json));
		newModelInput = '';
		fetchError = null;
		fetchSuccessMessage = null;
		isModalOpen = true;
	}

	function addModel(name: string) {
		const trimmed = name.trim();
		if (!trimmed) return;
		if (!formModels.some((m) => m.model_id === trimmed)) {
			formModels = [...formModels, parseModelItem(trimmed)];
		}
		newModelInput = '';
	}

	function removeModel(id: string) {
		formModels = formModels.filter((m) => m.id !== id);
	}

	function clearAllModels() {
		formModels = [];
	}

	async function fetchModelsLive() {
		if (!formBaseUrl.trim()) return;
		isFetchingModels = true;
		fetchError = null;
		fetchSuccessMessage = null;

		try {
			const res = await api.providers.fetchModels(
				formBaseUrl.trim(),
				formApiKey.trim() || undefined
			);
			if (res.models && res.models.length > 0) {
				const existingIds = new Set(formModels.map((m) => m.model_id));
				const newItems: FormModelItem[] = [];
				for (const mid of res.models) {
					if (!existingIds.has(mid)) {
						newItems.push(parseModelItem(mid));
					}
				}
				formModels = [...formModels, ...newItems];
				fetchSuccessMessage = `Discovered ${res.models.length} model${res.models.length === 1 ? '' : 's'} from endpoint`;
			} else {
				fetchError = 'No models returned by provider endpoint.';
			}
		} catch (e: any) {
			fetchError = e?.message || String(e);
		} finally {
			isFetchingModels = false;
		}
	}

	function formatTokens(count?: number): string {
		if (!count) return '';
		if (count >= 1000000) return `${(count / 1000000).toFixed(count % 1000000 === 0 ? 0 : 1)}m`;
		if (count >= 1000) return `${Math.round(count / 1000)}k`;
		return `${count}`;
	}

	function getModelBadges(cfgJson?: string): { ctx?: string; think?: string } {
		if (!cfgJson) return {};
		try {
			const p = JSON.parse(cfgJson);
			const ctx = p.context_window ? `${formatTokens(Number(p.context_window))} ctx` : undefined;
			const think = p.thinking ? 'thinking' : undefined;
			return { ctx, think };
		} catch {
			return {};
		}
	}

	async function saveProvider() {
		if (!formName.trim() || !formBaseUrl.trim()) return;
		isSaving = true;

		const payloadModels = formModels.map(serializeModelItem);

		try {
			if (editingProvider) {
				const updated = await api.providers.update(editingProvider.id, {
					name: formName.trim(),
					base_url: formBaseUrl.trim(),
					api_key: formApiKey.trim() || undefined,
					models: payloadModels
				});
				providers = providers.map((p) => (p.id === updated.id ? updated : p));
			} else {
				const created = await api.providers.create({
					name: formName.trim(),
					base_url: formBaseUrl.trim(),
					api_key: formApiKey.trim() || undefined,
					models: payloadModels
				});
				providers = [...providers, created];
			}
			isModalOpen = false;
		} catch (e: any) {
			alert('Failed to save provider: ' + (e?.message || e));
		} finally {
			isSaving = false;
		}
	}

	async function deleteProvider(id: string) {
		if (!confirm('Are you sure you want to delete this provider?')) return;
		try {
			await api.providers.delete(id);
			providers = providers.filter((p) => p.id !== id);
		} catch (e: any) {
			alert('Failed to delete provider: ' + (e?.message || e));
		}
	}
</script>

<div id="providers-layout" class="flex h-full flex-col overflow-hidden">
	<!-- Header -->
	<header
		id="providers-header"
		class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5"
	>
		<div class="flex items-center gap-3">
			<a
				href="/"
				onclick={() => navState.showLevel1()}
				class="text-zinc-400 hover:text-zinc-800 dark:hover:text-white transition-colors"
			>
				<ArrowLeft class="h-4 w-4" />
			</a>
			<div class="flex items-center gap-2">
				<Cpu class="h-4 w-4 text-indigo-500" />
				<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">
					LLM Providers & Models
				</h1>
			</div>
		</div>

		<button
			type="button"
			class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98"
			onclick={openCreateModal}
		>
			<Plus class="h-3.5 w-3.5" />
			<span>Add Provider</span>
		</button>
	</header>

	<!-- Table Area -->
	<main id="providers-content" class="flex-1 overflow-y-auto p-6">
		{#if isLoading}
			<div class="py-12 text-center text-xs text-zinc-400">Loading providers...</div>
		{:else if providers.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<div
					class="flex h-12 w-12 items-center justify-center rounded-2xl bg-zinc-100 dark:bg-zinc-800 text-zinc-400 mb-3"
				>
					<Cpu class="h-6 w-6" />
				</div>
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">No Providers Added</h3>
				<p class="mt-1 max-w-sm text-xs text-zinc-500 dark:text-zinc-400">
					Connect an OpenAI-compatible endpoint like Local Llama, Ollama, DeepSeek, or OpenAI, then
					fetch and configure available models with context windows and thinking mode.
				</p>
				<button
					type="button"
					class="mt-4 rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 shadow-xs"
					onclick={openCreateModal}
				>
					Add First Provider
				</button>
			</div>
		{:else}
			<section
				id="providers-table-section"
				class="overflow-x-auto rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 shadow-xs"
			>
				<table class="w-full text-left text-xs">
					<thead
						class="border-b border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-500 dark:text-zinc-400 font-medium"
					>
						<tr>
							<th class="p-2.5 pl-3 md:p-3 md:pl-4">Provider Name</th>
							<th class="p-2.5 md:p-3">Base URL</th>
							<th class="p-2.5 md:p-3">API Key</th>
							<th class="hidden p-3 md:table-cell">Configured Models</th>
							<th class="p-2.5 pr-3 md:p-3 md:pr-4 text-right">Actions</th>
						</tr>
					</thead>
					<tbody
						class="divide-y divide-zinc-200/60 dark:divide-zinc-800/60 text-zinc-700 dark:text-zinc-300"
					>
						{#each providers as prov (prov.id)}
							<tr class="hover:bg-zinc-50/70 dark:hover:bg-zinc-800/50 transition-colors">
								<td class="p-2.5 pl-3 md:p-3 md:pl-4 font-medium text-zinc-900 dark:text-white">{prov.name}</td>
								<td class="p-2.5 md:p-3 font-mono text-zinc-500 dark:text-zinc-400 break-all">{prov.base_url}</td>
								<td class="p-3">
									{#if prov.api_key}
										<span
											class="inline-flex items-center gap-1 text-[11px] font-mono text-zinc-600 dark:text-zinc-400"
										>
											<Key class="h-3 w-3 text-emerald-500" />
											Configured
										</span>
									{:else}
										<span class="text-[11px] text-zinc-400 italic">None (Local)</span>
									{/if}
								</td>
								<td class="hidden md:table-cell p-3">
									{#if !prov.models || prov.models.length === 0}
										<span class="text-zinc-400 italic">No models configured</span>
									{:else}
										<div class="flex flex-wrap items-center gap-1.5">
											{#each prov.models.slice(0, 4) as m}
												{@const badges = getModelBadges(m.config_json)}
												<span
													class="inline-flex items-center gap-1 rounded-lg bg-zinc-100 dark:bg-zinc-800/90 px-2 py-0.5 font-mono text-[11px] font-medium text-zinc-800 dark:text-zinc-200 border border-zinc-200/60 dark:border-zinc-700/60"
												>
													<span>{m.model_id}</span>
													{#if badges.ctx}
														<span class="text-[10px] text-zinc-400">[{badges.ctx}]</span>
													{/if}
													{#if badges.think}
														<span class="text-[10px] text-purple-600 dark:text-purple-400 font-sans">🧠</span>
													{/if}
												</span>
											{/each}
											{#if prov.models.length > 4}
												<span class="text-[11px] font-medium text-zinc-400">
													+{prov.models.length - 4} more
												</span>
											{/if}
										</div>
									{/if}
								</td>
								<td class="p-2.5 pr-3 md:p-3 md:pr-4 text-right">
									<div class="flex items-center justify-end gap-1">
										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-800 dark:hover:text-white transition-colors"
											onclick={() => openEditModal(prov)}
											title="Edit"
										>
											<Edit3 class="h-3.5 w-3.5" />
										</button>
										<button
											type="button"
											class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
											onclick={() => deleteProvider(prov.id)}
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

<!-- Modal: Add/Edit Provider -->
{#if isModalOpen}
	<div
		id="modal-provider"
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
	>
		<div
			class="w-full max-w-xl rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl overflow-hidden flex flex-col max-h-[90vh]"
		>
			<div
				class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3"
			>
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
					{editingProvider ? 'Edit Provider' : 'Add New Provider'}
				</h3>
				<button
					type="button"
					class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
					onclick={() => (isModalOpen = false)}
				>
					<X class="h-4 w-4" />
				</button>
			</div>

			<form
				onsubmit={(e) => {
					e.preventDefault();
					saveProvider();
				}}
				class="mt-4 space-y-4 overflow-y-auto pr-1 flex-1"
			>
				<div class="grid grid-cols-2 gap-3">
					<div>
						<label
							for="p-name"
							class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1"
						>
							Provider Name
						</label>
						<input
							id="p-name"
							type="text"
							bind:value={formName}
							placeholder="e.g. OpenAI, DeepSeek, Ollama"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
							required
						/>
					</div>

					<div>
						<label
							for="p-api-key"
							class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1"
						>
							API Key
						</label>
						<input
							id="p-api-key"
							type="password"
							bind:value={formApiKey}
							placeholder="sk-... (Optional for local)"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						/>
					</div>
				</div>

				<div>
					<label
						for="p-base-url"
						class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1"
					>
						Base URL
					</label>
					<input
						id="p-base-url"
						type="url"
						bind:value={formBaseUrl}
						placeholder="https://api.openai.com/v1"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						required
					/>
				</div>

				<!-- Provider Models Section -->
				<div
					class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/50 p-3.5 space-y-3"
				>
					<div class="flex items-center justify-between">
						<div>
							<div class="flex items-center gap-1.5">
								<Layers class="h-3.5 w-3.5 text-indigo-500" />
								<span class="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
									Provider Models & Advanced Config
								</span>
							</div>
							<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-0.5">
								Configure context window, reasoning/thinking, and sampling per model.
							</p>
						</div>

						<button
							type="button"
							class="flex items-center gap-1.5 rounded-lg border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 px-2.5 py-1.5 text-[11px] font-medium text-zinc-800 dark:text-zinc-200 hover:bg-zinc-50 dark:hover:bg-zinc-750 transition-colors disabled:opacity-50"
							onclick={fetchModelsLive}
							disabled={isFetchingModels || !formBaseUrl.trim()}
						>
							<RefreshCw class="h-3 w-3 {isFetchingModels ? 'animate-spin' : ''}" />
							<span>{isFetchingModels ? 'Fetching...' : 'Fetch Available Models'}</span>
						</button>
					</div>

					{#if fetchSuccessMessage}
						<div
							class="flex items-center gap-1.5 rounded-lg bg-emerald-50 dark:bg-emerald-950/40 border border-emerald-200 dark:border-emerald-800 px-2.5 py-1.5 text-[11px] text-emerald-700 dark:text-emerald-300"
						>
							<Check class="h-3.5 w-3.5 shrink-0 text-emerald-500" />
							<span>{fetchSuccessMessage}</span>
						</div>
					{/if}

					{#if fetchError}
						<div
							class="flex items-center gap-1.5 rounded-lg bg-rose-50 dark:bg-rose-950/40 border border-rose-200 dark:border-rose-800 px-2.5 py-1.5 text-[11px] text-rose-700 dark:text-rose-300"
						>
							<AlertCircle class="h-3.5 w-3.5 shrink-0 text-rose-500" />
							<span>{fetchError}</span>
						</div>
					{/if}

					<!-- Manual Model Input -->
					<div class="flex items-center gap-2">
						<input
							type="text"
							bind:value={newModelInput}
							onkeydown={(e) => {
								if (e.key === 'Enter') {
									e.preventDefault();
									addModel(newModelInput);
								}
							}}
							placeholder="Add model name (e.g. gpt-4o, google/gemini-2.5-flash)"
							class="flex-1 rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs font-mono text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
						/>
						<button
							type="button"
							class="rounded-lg bg-zinc-900 dark:bg-white px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40"
							onclick={() => addModel(newModelInput)}
							disabled={!newModelInput.trim()}
						>
							Add
						</button>
					</div>

					<!-- Model List Header -->
					<div class="flex items-center justify-between text-[11px] text-zinc-500 pt-1">
						<span>Configured Models ({formModels.length})</span>
						{#if formModels.length > 0}
							<button
								type="button"
								class="text-zinc-400 hover:text-rose-500 transition-colors"
								onclick={clearAllModels}
							>
								Clear all
							</button>
						{/if}
					</div>

					{#if formModels.length === 0}
						<div
							class="rounded-lg border border-dashed border-zinc-200 dark:border-zinc-800 p-4 text-center text-[11px] text-zinc-400"
						>
							No models added yet. Click "Fetch Available Models" or type a model name above.
						</div>
					{:else}
						<!-- Expandable Model Cards List -->
						<div class="space-y-2 max-h-72 overflow-y-auto pr-1">
							{#each formModels as item (item.id)}
								<div
									class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900/90 shadow-2xs overflow-hidden transition-all"
								>
									<!-- Row Header -->
									<div
										class="flex items-center justify-between p-2.5 hover:bg-zinc-50 dark:hover:bg-zinc-800/40"
									>
										<button
											type="button"
											class="flex items-center gap-2 truncate text-left flex-1 cursor-pointer hover:opacity-80 transition-opacity"
											onclick={() => (item.isExpanded = !item.isExpanded)}
											aria-expanded={item.isExpanded}
										>
											<span class="font-mono text-xs font-semibold text-zinc-900 dark:text-zinc-100 truncate">
												{item.model_id}
											</span>
											{#if item.name}
												<span class="text-[11px] text-zinc-400 truncate">({item.name})</span>
											{/if}

											<!-- Status Badges -->
											<div class="flex items-center gap-1 shrink-0">
												{#if item.context_window}
													<span
														class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 border border-zinc-200 dark:border-zinc-700"
													>
														{formatTokens(item.context_window)} ctx
													</span>
												{/if}
												{#if item.thinking_enabled}
													<span
														class="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] font-mono bg-purple-50 dark:bg-purple-950/60 text-purple-700 dark:text-purple-300 border border-purple-200 dark:border-purple-800"
													>
														<Sparkles class="h-2.5 w-2.5" />
														{item.thinking_budget ? `${formatTokens(item.thinking_budget)} think` : 'think'}
													</span>
												{/if}
											</div>
										</button>

										<div class="flex items-center gap-1 shrink-0 ml-2">
											<button
												type="button"
												class="flex items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-900 dark:hover:text-white transition-colors"
												onclick={() => (item.isExpanded = !item.isExpanded)}
												title="Configure advanced settings"
												aria-label="Configure advanced settings"
											>
												<Sliders class="h-3 w-3" />
												<span>Config</span>
												<ChevronDown
													class="h-3 w-3 transition-transform {item.isExpanded ? 'rotate-180' : ''}"
												/>
											</button>
											<button
												type="button"
												class="rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-rose-500 transition-colors"
												onclick={() => removeModel(item.id)}
												title="Remove model"
												aria-label="Remove model"
											>
												<Trash2 class="h-3.5 w-3.5" />
											</button>
										</div>
									</div>

									<!-- Expanded Advanced Configuration Panel -->
									{#if item.isExpanded}
										<div
											class="border-t border-zinc-100 dark:border-zinc-800 bg-zinc-50/70 dark:bg-zinc-950/70 p-3.5 space-y-3.5 text-xs"
										>
											<div class="grid grid-cols-2 gap-3">
												<div>
													<label
														for="model-name-{item.id}"
														class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300 mb-1"
													>
														Friendly Display Name
													</label>
													<input
														id="model-name-{item.id}"
														type="text"
														bind:value={item.name}
														placeholder="e.g. Gemini Flash Thinking"
														class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
													/>
												</div>

												<div>
													<label
														for="model-ctx-{item.id}"
														class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300 mb-1"
													>
														Max Context Window (tokens)
													</label>
													<input
														id="model-ctx-{item.id}"
														type="number"
														bind:value={item.context_window}
														placeholder="e.g. 128000, 1000000"
														class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs font-mono text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
													/>
												</div>
											</div>

											<div class="grid grid-cols-2 gap-3">
												<div>
													<label
														for="model-maxtokens-{item.id}"
														class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300 mb-1"
													>
														Max Output Tokens
													</label>
													<input
														id="model-maxtokens-{item.id}"
														type="number"
														bind:value={item.max_tokens}
														placeholder="e.g. 4096, 8192"
														class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs font-mono text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
													/>
												</div>

												<div>
													<div class="flex items-center justify-between mb-1">
														<label
															for="model-temp-{item.id}"
															class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300"
														>
															Temperature
														</label>
														<span class="text-[11px] font-mono text-zinc-500">
															{item.temperature ?? 0.7}
														</span>
													</div>
													<input
														id="model-temp-{item.id}"
														type="range"
														min="0"
														max="2"
														step="0.05"
														bind:value={item.temperature}
														class="w-full accent-indigo-500"
													/>
												</div>
											</div>

											<!-- Thinking / Reasoning Configuration -->
											<div
												class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-3 space-y-2.5"
											>
												<div class="flex items-center justify-between">
													<div class="flex items-center gap-2">
														<Sparkles class="h-3.5 w-3.5 text-purple-500" />
														<div>
															<span
																class="block text-xs font-semibold text-zinc-900 dark:text-white"
															>
																Thinking / Reasoning Mode
															</span>
															<span class="text-[10px] text-zinc-500 dark:text-zinc-400">
																For reasoning models (Claude 3.7, DeepSeek R1, Gemini Flash Thinking, o1)
															</span>
														</div>
													</div>

													<button
														type="button"
														aria-label="Toggle thinking mode"
														class="relative inline-flex h-4 w-7 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {item.thinking_enabled
															? 'bg-purple-600'
															: 'bg-zinc-300 dark:bg-zinc-700'}"
														onclick={() => (item.thinking_enabled = !item.thinking_enabled)}
													>
														<span
															class="pointer-events-none inline-block h-3 w-3 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {item.thinking_enabled
																? 'translate-x-3'
																: 'translate-x-0'}"
														></span>
													</button>
												</div>

												{#if item.thinking_enabled}
													<div
														class="pt-1.5 border-t border-zinc-100 dark:border-zinc-800 flex items-center gap-3"
													>
														<div class="flex-1">
															<label
																for="model-thinkbudget-{item.id}"
																class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300 mb-1"
															>
																Thinking Budget Tokens (budget_tokens)
															</label>
															<input
																id="model-thinkbudget-{item.id}"
																type="number"
																bind:value={item.thinking_budget}
																placeholder="e.g. 2048, 4096, 8192"
																class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-2.5 py-1.5 text-xs font-mono text-zinc-900 dark:text-white focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
															/>
														</div>
													</div>
												{/if}
											</div>

											<!-- Custom Parameters (Raw JSON) -->
											<div>
												<label
													for="model-json-{item.id}"
													class="block text-[11px] font-medium text-zinc-700 dark:text-zinc-300 mb-1"
												>
													Additional Parameters (JSON)
												</label>
												<textarea
													id="model-json-{item.id}"
													rows="2"
													bind:value={item.custom_json}
													placeholder={'{\n  "reasoning_effort": "high"\n}'}
													class="w-full rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 p-2 text-[11px] font-mono text-zinc-900 dark:text-white focus:outline-none focus:border-zinc-400 dark:focus:border-zinc-600"
												></textarea>
											</div>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
				</div>

				<div
					class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3"
				>
					<button
						type="button"
						class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
						onclick={() => (isModalOpen = false)}
					>
						Cancel
					</button>
					<button
						type="submit"
						class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
						disabled={isSaving}
					>
						{isSaving ? 'Saving...' : editingProvider ? 'Update Provider' : 'Create Provider'}
					</button>
				</div>
			</form>
		</div>
	</div>
{/if}
