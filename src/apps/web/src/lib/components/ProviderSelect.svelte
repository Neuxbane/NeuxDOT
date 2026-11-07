<script lang="ts">
	import type { Provider, ProviderModel } from '$lib/api';
	import { Check, X, Search, ChevronDown, Layers } from 'lucide-svelte';

	interface Props {
		allProviders: Provider[];
		selectedIds: string[];
		onchange?: (selected: string[]) => void;
	}

	let { allProviders = [], selectedIds = $bindable([]), onchange }: Props = $props();

	let isOpen = $state(false);
	let query = $state('');

	function findModelInfo(modelIdPk: string): { provider: Provider; model: ProviderModel } | null {
		for (const prov of allProviders) {
			if (prov.models) {
				const found = prov.models.find((m) => m.id === modelIdPk);
				if (found) return { provider: prov, model: found };
			}
		}
		return null;
	}

	const matchingProviders = $derived(
		allProviders
			.map((prov) => {
				const q = query.toLowerCase().trim();
				if (!q) return prov;

				const matchesProvName = prov.name.toLowerCase().includes(q);
				const matchedModels = (prov.models || []).filter((m) =>
					m.model_id.toLowerCase().includes(q)
				);

				if (matchesProvName) return prov;
				if (matchedModels.length > 0) {
					return { ...prov, models: matchedModels };
				}
				return null;
			})
			.filter((p): p is Provider => p !== null)
	);

	function toggleModel(modelIdPk: string) {
		if (selectedIds.includes(modelIdPk)) {
			selectedIds = selectedIds.filter((i) => i !== modelIdPk);
		} else {
			selectedIds = [...selectedIds, modelIdPk];
		}
		onchange?.(selectedIds);
	}

	function removeModel(modelIdPk: string) {
		selectedIds = selectedIds.filter((i) => i !== modelIdPk);
		onchange?.(selectedIds);
	}
</script>

<div class="relative w-full">
	<!-- Selected Chips Display & Input Trigger -->
	<div
		class="min-h-10 flex flex-wrap items-center gap-1.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900/60 p-2 text-sm focus-within:border-zinc-400 dark:focus-within:border-zinc-600 shadow-xs transition-colors"
	>
		{#if selectedIds.length === 0}
			<span class="text-xs text-zinc-400 dark:text-zinc-500 pl-1">
				Select models / providers to connect...
			</span>
		{/if}

		{#each selectedIds as id}
			{@const info = findModelInfo(id)}
			{#if info}
				<span
					class="inline-flex items-center gap-1.5 rounded-lg bg-zinc-100 dark:bg-zinc-800 px-2.5 py-1 text-xs font-medium text-zinc-800 dark:text-zinc-200 border border-zinc-200 dark:border-zinc-700/80 shadow-2xs"
				>
					<span class="text-zinc-500 dark:text-zinc-400 font-normal">{info.provider.name}</span>
					<span class="text-zinc-900 dark:text-white font-mono text-[11px] font-semibold">
						{info.model.model_id}
					</span>
					<button
						type="button"
						class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white ml-0.5"
						onclick={() => removeModel(id)}
						aria-label="Remove model {info.model.model_id}"
					>
						<X class="h-3 w-3" />
					</button>
				</span>
			{/if}
		{/each}

		<button
			type="button"
			class="ml-auto text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 p-1 rounded-lg"
			onclick={() => (isOpen = !isOpen)}
			aria-label="Toggle model dropdown"
		>
			<ChevronDown class="h-4 w-4" />
		</button>
	</div>

	<!-- Dropdown Menu -->
	{#if isOpen}
		<div
			class="absolute left-0 right-0 z-50 mt-1 max-h-64 overflow-y-auto rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-2 shadow-xl"
		>
			<div
				class="mb-2 flex items-center gap-2 border-b border-zinc-100 dark:border-zinc-800 pb-2 px-1"
			>
				<Search class="h-3.5 w-3.5 text-zinc-400 dark:text-zinc-500" />
				<input
					type="text"
					bind:value={query}
					placeholder="Search providers or models..."
					class="w-full bg-transparent text-xs text-zinc-800 dark:text-zinc-200 focus:outline-none placeholder-zinc-400"
				/>
			</div>

			{#if matchingProviders.length === 0}
				<div class="py-3 text-center text-xs text-zinc-400 italic">No matching providers or models</div>
			{:else}
				<div class="space-y-3">
					{#each matchingProviders as prov}
						<div class="space-y-1">
							<div
								class="px-2 pt-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500 flex items-center gap-1.5"
							>
								<Layers class="h-3 w-3 text-indigo-500" />
								<span>{prov.name}</span>
							</div>

							{#if !prov.models || prov.models.length === 0}
								<div class="px-2.5 py-1 text-[11px] text-zinc-400 italic">No models configured</div>
							{:else}
								{#each prov.models as m}
									{@const isSelected = selectedIds.includes(m.id)}
									<button
										type="button"
										class="flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-xs text-left transition-colors {isSelected
											? 'bg-zinc-100 dark:bg-zinc-800 text-zinc-900 dark:text-white font-medium'
											: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-50 dark:hover:bg-zinc-800/50'}"
										onclick={() => toggleModel(m.id)}
									>
										<span class="font-mono text-[11px]">{m.model_id}</span>
										{#if isSelected}
											<Check class="h-3.5 w-3.5 text-zinc-900 dark:text-white" />
										{/if}
									</button>
								{/each}
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</div>
